import AxeBuilder from "@axe-core/playwright";
import { expect, test, type Page } from "@playwright/test";

import { id, mockApi, org, ws } from "./support/mock-api";
import { overflow } from "./support/open";

// P5-IA-2 (C-963..C-965, C-986, C-989; docs/14 §8): Internet access is one page at each level, with one Apply.

const policyOf = (rules: unknown[], status: string, revision: number | null) => ({
  id: id(80), number: 1, status, created_at: "2026-01-01T00:00:00Z", default_action: "none", open_lane_posture: "enabled",
  uncredentialed_default: "none", uncredentialed_allowlist: [], draft_revision: revision,
  rules: rules.map((r, i) => ({ priority: i, description: null, rate_limit: null, conditions: [], identities: [], enabled: true, require_approval: false, logical_id: id(500 + i), ...(r as object) })),
});

/** A policy that keeps what is written to the draft and copies it when published, like the server. */
async function statefulPolicy(page: Page, base: string, scope: string) {
  let draft: Record<string, unknown>[] = [];
  let published: Record<string, unknown>[] = [];
  let rev = 1;
  const publishedBodies: unknown[] = [];
  await page.route(`**${base}**`, async (route) => {
    const req = route.request();
    const p = new URL(req.url()).pathname;
    const json = (b: unknown) => route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify(b) });
    if (p === base && req.method() === "GET") return json({ scope, published: policyOf(published, "published", null), draft: policyOf(draft, "draft", rev) });
    if (p === `${base}/warnings`) return json({ items: [] });
    if (p === `${base}/draft` && req.method() === "PUT") {
      draft = (req.postDataJSON() as { rules: Record<string, unknown>[] }).rules;
      rev += 1;
      return json(policyOf(draft, "draft", rev));
    }
    if (p === `${base}/publish` && req.method() === "POST") {
      publishedBodies.push(req.postDataJSON());
      published = draft;
      return json({ scope, published: policyOf(published, "published", null), draft: policyOf(draft, "draft", rev) });
    }
    return route.fallback();
  });
  return { published: publishedBodies };
}

async function open(page: Page, path: string, scheme: "light" | "dark" = "light") {
  const errors: string[] = [];
  page.on("pageerror", (e) => errors.push(e.message));
  await page.emulateMedia({ colorScheme: scheme });
  await page.goto(path);
  await page.getByRole("heading", { level: 1, name: "Internet access" }).waitFor();
  expect(errors, `${path} threw`).toEqual([]);
}

test.describe("Internet access (mocked API)", () => {
  test.use({ viewport: { width: 1280, height: 900 } });

  test("a workspace owner sees the summary, two Common changes and no default controls until Tighten… is opened", async ({ page }) => {
    await mockApi(page);
    await statefulPolicy(page, `/v1/workspaces/${ws.id}/policy`, "workspace");
    await open(page, "/o/acme/w/main/internet-access");
    await expect(page.getByTestId("internet-summary")).toHaveText("Agents here can reach any website. Nothing asks you first.");
    await expect(page.getByRole("button", { name: /Ask before changes on a website/ })).toBeVisible();
    await expect(page.getByRole("button", { name: /Block a website/ })).toBeVisible();
    await expect(page.getByText("No rules yet. Until you add one, requests follow the limits and defaults below.")).toBeVisible();
    await expect(page.getByLabel("Requests without an account")).toHaveCount(0);
    await expect(page.getByLabel("Browser connections (not inspected)")).toHaveCount(0);
    await expect(page.getByLabel("Default rule")).toHaveCount(0);
    await page.getByRole("button", { name: "Tighten…" }).click();
    await expect(page.getByLabel("Requests without an account")).toBeVisible();
    await expect(page.getByLabel("Browser connections (not inspected)")).toBeVisible();
    await expect(page.getByLabel("Default rule")).toBeVisible();
    // One bar, and none until something is pending.
    await expect(page.getByRole("region", { name: "Unsaved changes" })).toHaveCount(0);
    expect(await overflow(page)).toBeLessThanOrEqual(0);
  });

  test("a workspace owner blocks a website with a Common change and Apply", async ({ page }) => {
    await mockApi(page);
    const server = await statefulPolicy(page, `/v1/workspaces/${ws.id}/policy`, "workspace");
    await open(page, "/o/acme/w/main/internet-access");
    await page.getByRole("button", { name: /Block a website/ }).click();
    await page.getByLabel("Website", { exact: true }).fill("pastebin.com");
    await page.getByRole("button", { name: "Show the rule" }).click();
    await expect(page.getByTestId("common-change-rule")).toContainText("Block pastebin.com");
    await page.getByRole("button", { name: "Add rule" }).click();
    await expect(page.getByText("Not applied yet", { exact: true })).toBeVisible();
    const bar = page.getByRole("region", { name: "Unsaved changes" });
    await expect(bar).toContainText("1 unsaved change");
    await bar.getByRole("button", { name: "Save", exact: true }).click();
    await expect.poll(() => server.published.length).toBe(1);
    await expect(page.getByText("Not applied yet", { exact: true })).toHaveCount(0);
    await expect(page.getByText("Block pastebin.com")).toBeVisible();
    await expect(bar).toHaveCount(0);
  });

  test("an org admin tightens the websites agents may reach; a workspace's page shows it as a limit from above", async ({ page }) => {
    await mockApi(page);
    let posture = "open";
    const doc = () => ({
      document: { byo: { posture: null, source_cidrs: [], require_context_credentials: false }, agent_tokens: { max_age_days: null }, llm: { posture: "open", endpoints: {} }, egress: { posture, open_lane: "enabled", allowlist: [] } },
      revision: posture === "open" ? 1 : 2,
    });
    const sourced = (value: string, source: string) => ({ value, source });
    await page.route("**/v1/**", async (route) => {
      const req = route.request();
      const p = new URL(req.url()).pathname;
      const json = (b: unknown) => route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify(b) });
      if (p === `/v1/orgs/${org.id}/governance` && req.method() === "GET") return json(doc());
      if (p === `/v1/orgs/${org.id}/governance` && req.method() === "PUT") {
        posture = (req.postDataJSON() as { document: { egress: { posture: string } } }).document.egress.posture;
        return json(doc());
      }
      if (p === `/v1/orgs/${org.id}/governance/effective`) {
        return json({
          byo: { posture: sourced("enabled", "instance"), require_context_credentials: sourced("false", "instance"), source_cidrs: { value: null, source: "instance" } },
          egress: { baseline_allowlist: [], org_allowlist: [{ host: "pypi.org" }], open_lane: sourced("enabled", "org"), posture: sourced(posture, "org") },
          llm: { endpoints: [], posture: sourced("open", "org") },
          agent_tokens: { max_age_days: { value: null, source: "instance" } },
          non_compliant_secrets: [],
        });
      }
      return route.fallback();
    });
    await statefulPolicy(page, `/v1/orgs/${org.id}/policy`, "organization");
    await statefulPolicy(page, `/v1/workspaces/${ws.id}/policy`, "workspace");
    await open(page, "/o/acme/internet-access");
    await page.getByLabel("Which websites agents may reach").selectOption("credentialed_only");
    const bar = page.getByRole("region", { name: "Unsaved changes" });
    await expect(bar).toContainText("1 unsaved change");
    await bar.getByRole("button", { name: "Save", exact: true }).click();
    await expect(bar).toHaveCount(0);
    await expect.poll(() => posture).toBe("credentialed_only");
    await open(page, "/o/acme/w/main/internet-access");
    const limits = page.getByRole("region", { name: "Limits from above" });
    await expect(limits.getByText("Websites agents may reach")).toBeVisible();
    await expect(limits.getByText("Only listed websites")).toBeVisible();
    await expect(limits.getByText("Set by your organization").first()).toBeVisible();
    await expect(page.getByTestId("internet-summary")).toContainText("can reach only the websites on the allow-list");
  });

  test("a workspace that tightened appears in the organization's table", async ({ page }) => {
    await mockApi(page);
    await statefulPolicy(page, `/v1/orgs/${org.id}/policy`, "organization");
    await open(page, "/o/acme/internet-access");
    const row = page.getByRole("row").filter({ hasText: "Main workspace" });
    await expect(row).toContainText("Blocked unless listed");
    await expect(row).toContainText("Turned off");
    await expect(row.getByRole("link", { name: "Open" })).toHaveAttribute("href", "/o/acme/w/main/internet-access");
  });

  // FIX-164 (D-365): a member of the workspace reads its published rules, read-only, from the member route.
  for (const scheme of ["light", "dark"] as const) {
    for (const width of [1280, 390]) {
      test(`a workspace member reads the summary and the published rules with no controls, ${scheme} theme, ${width} px`, async ({ page }) => {
        await page.setViewportSize({ width, height: width === 1280 ? 900 : 844 });
        await mockApi(page);
        const requested: string[] = [];
        await page.route("**/v1/**", async (route) => {
          const p = new URL(route.request().url()).pathname;
          requested.push(p);
          const json = (b: unknown) => route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify(b) });
          if (p === `/v1/workspaces/${ws.id}`) return json({ ...ws, my_role: "member", contacts: { owners: [{ name: "Bob Builder" }] } });
          if (p === `/v1/workspaces/${ws.id}/internet-access`) {
            const g = policyOf([{ name: "No evil", action: "block", targets: [{ type: "network", host: "evil.example.com" }] }], "published", null);
            return json({ number: 2, default_action: g.default_action, uncredentialed_default: g.uncredentialed_default, uncredentialed_allowlist: [], open_lane_posture: "enabled", rules: g.rules });
          }
          if (p === `/v1/workspaces/${ws.id}/policy`) return route.fulfill({ status: 403, contentType: "application/json", body: JSON.stringify({ error: { code: "forbidden", message: "no" } }) });
          return route.fallback();
        });
        await open(page, "/o/acme/w/main/internet-access", scheme);
        await expect(page.getByTestId("internet-summary")).toHaveText("Agents here can reach any website, except where a rule blocks it. Nothing asks you first.");
        const rules = page.getByRole("list", { name: "Rules people wrote" });
        await expect(rules).toContainText("No evil");
        await expect(rules).toContainText("evil.example.com");
        await expect(page.getByText("Only workspace owners can change this. Ask Bob Builder.")).toBeVisible();
        for (const name of [/Block a website/, /Ask before changes on a website/, /Add rule/, /Tighten/]) {
          await expect(page.getByRole("button", { name })).toHaveCount(0);
        }
        expect(requested.filter((p) => p.endsWith("/policy") || p.endsWith("/policy/warnings") || p.endsWith("/policy/generations"))).toEqual([]);
        expect(await overflow(page), "scrolls sideways").toBeLessThanOrEqual(0);
        const { violations } = await new AxeBuilder({ page }).analyze();
        expect(violations.filter((v) => v.impact === "serious" || v.impact === "critical").map((v) => v.id)).toEqual([]);
      });
    }
  }

  for (const scheme of ["light", "dark"] as const) {
    test(`the three pages fit at 390 px, ${scheme} theme`, async ({ page }) => {
      await page.setViewportSize({ width: 390, height: 844 });
      await mockApi(page);
      await statefulPolicy(page, `/v1/workspaces/${ws.id}/policy`, "workspace");
      await statefulPolicy(page, `/v1/orgs/${org.id}/policy`, "organization");
      for (const path of ["/admin/internet-access", "/o/acme/internet-access", "/o/acme/w/main/internet-access"]) {
        await open(page, path, scheme);
        await expect.poll(() => page.locator("html").getAttribute("data-theme"), { timeout: 20_000 }).toBe(scheme);
        expect(await overflow(page), `${path} scrolls sideways`).toBeLessThanOrEqual(0);
      }
    });
  }
});
