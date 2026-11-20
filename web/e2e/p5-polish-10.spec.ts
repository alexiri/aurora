import { expect, test, type Page } from "@playwright/test";

import { id, mockApi, setupStatus } from "./support/mock-api";
import { overflow, type Scheme } from "./support/open";
import { serious } from "./support/axe";

// P5-POLISH-10 (D-391, E4/E5, D-381, C-1032), API mocked: a fresh install walks the five-step checklist; adding the AI
// key stays on it with a toast; the apps list is Waiting + From Aurora and its rows lead to a review; Approve works
// without opening any section and returns to the list. Both themes at 1280 and 390 px, axe, no console errors.

const now = "2026-11-19T10:00:00Z";
const UPLOADED = id(900);

const pkg = (pid: string, name: string, over: Record<string, unknown> = {}) => ({
  id: pid,
  package_id: `${name.toLowerCase().replace(/\W+/g, "-")}-pkg`,
  version: "1.0.0",
  contract: 1,
  publisher: "aurora",
  digest: "d".repeat(64),
  signer: { kind: "builtin" },
  source: "builtin",
  origin: "aurora",
  summary: null,
  reaches: [`api.${name.split(" ")[0]!.toLowerCase()}.test`],
  reaches_more: 0,
  apps: [{ app_id: name.toLowerCase(), name }],
  status: "active",
  status_reason: null,
  violations: [],
  widenings: [],
  host_claims: [],
  permissions: { hooks: ["token.mint"], egress: { "token.mint": ["{instance}"] }, context: [] },
  impact: null,
  approved_by: null,
  approved_at: null,
  created_at: now,
  ...over,
});

const AURORA = ["GitHub", "Google Workspace", "Sentry", "Slack", "Jira"].map((n, i) => pkg(id(910 + i), n));

interface State {
  keyAdded: boolean;
  approved: boolean;
}

/** The API mock, with the few things this brief changes kept as state (the key, the approval). */
async function open(page: Page, path: string, scheme: Scheme, state: State = { keyAdded: false, approved: false }) {
  const errors: string[] = [];
  page.on("pageerror", (e) => errors.push(e.message));
  page.on("console", (m) => {
    if (m.type() === "error" && !/^Failed to load resource/.test(m.text())) errors.push(m.text());
  });
  await mockApi(page);
  const json = (b: unknown, status = 200) => ({ status, contentType: "application/json", body: JSON.stringify(b) });
  await page.route("**/v1/**", async (route) => {
    const url = new URL(route.request().url());
    const p = url.pathname;
    const method = route.request().method();
    if (p === "/v1/admin/setup") {
      // A fresh install that has chosen where people land: the key and the first answer are still to do.
      const steps = setupStatus.steps.map((s) => {
        if (s.id === "placement") return { ...s, status: "ok", reason: "placement_ok", checks: [] };
        if (s.id === "llm") {
          return { ...s, checks: [{ id: "org_key", status: state.keyAdded ? "ok" : "todo", reason: state.keyAdded ? "org_key_set" : "org_key_missing", message: "", value: null, helm: null }], status: state.keyAdded ? "ok" : "todo" };
        }
        if (s.id === "optional") {
          return { ...s, checks: [{ id: "email", status: "todo", reason: "email_not_configured", message: "E-mail is not set up.", value: null, helm: null }] };
        }
        return s;
      });
      return route.fulfill(json({ ...setupStatus, placement_saved: true, steps }));
    }
    if (p === "/v1/llm/providers") {
      return route.fulfill(
        json({
          providers: [{ id: "litellm", display_name: "LiteLLM", auth_modes: ["bearer"] }],
          endpoints: [{ id: "litellm", provider: "litellm", display_name: "AIGW", permitted: true, key_scopes: ["organization"] }],
        }),
      );
    }
    if (/^\/v1\/orgs\/[^/]+\/secrets$/.test(p)) {
      if (method === "POST") {
        state.keyAdded = true;
        return route.fulfill(json({ id: id(950), name: "AIGW key", kind: "llm", status: "active", scope: "organization", created_at: now }, 201));
      }
      return route.fulfill(
        json({ items: state.keyAdded ? [{ id: id(950), name: "AIGW key", kind: "llm", status: "active", created_at: now }] : [], next_cursor: null }),
      );
    }
    // Apps that ship no icon: the monogram tile (the mock reads icons from plugins/builtin and throws for any other id).
    if (/^\/v1\/apps\/[^/]+\/icon$/.test(p)) return route.fulfill({ status: 404, body: "" });
    if (p === "/v1/admin/addon-sources") return route.fulfill(json({ items: [] }));
    if (p === "/v1/admin/plugins" && method === "GET") {
      const uploaded = pkg(UPLOADED, "Acme tracker", {
        package_id: "acme-tracker",
        origin: "uploaded",
        source: "upload",
        publisher: "acme",
        signer: { kind: "unsigned" },
        status: state.approved ? "active" : "pending_approval",
        status_reason: state.approved ? null : "first_install",
        reaches: ["tracker.acme.test"],
      });
      return route.fulfill(json({ items: [uploaded, ...AURORA], next_cursor: null }));
    }
    if (p === `/v1/admin/plugins/${UPLOADED}` && method === "GET") {
      return route.fulfill(
        json(pkg(UPLOADED, "Acme tracker", { package_id: "acme-tracker", origin: "uploaded", source: "upload", signer: { kind: "unsigned" }, status: "pending_approval", status_reason: "first_install", reaches: ["tracker.acme.test"] })),
      );
    }
    if (p === `/v1/admin/plugins/${UPLOADED}/approve` && method === "POST") {
      state.approved = true;
      return route.fulfill(json({ ...pkg(UPLOADED, "Acme tracker", { status: "active" }), connections_reauth: 0 }));
    }
    return route.fallback();
  });
  await page.emulateMedia({ colorScheme: scheme });
  await page.goto(path);
  await page.locator("h1, h2").first().waitFor();
  await page.waitForLoadState("networkidle");
  await expect.poll(() => page.locator("html").getAttribute("data-theme"), { timeout: 20_000 }).toBe(scheme);
  expect(errors, `${path} threw`).toEqual([]);
  return { errors, state };
}

async function clean(page: Page, narrow: boolean) {
  expect(await serious(page)).toEqual([]);
  if (narrow) expect(await overflow(page)).toBeLessThanOrEqual(0);
}

for (const scheme of ["light", "dark"] as const) {
  for (const width of [1280, 390]) {
    test.describe(`${scheme} theme, ${width} px`, () => {
      test.use({ viewport: { width, height: 900 } });

      test("a fresh install walks the five steps; the AI key stays on the checklist with a toast", async ({ page }) => {
        const { errors } = await open(page, "/admin", scheme);
        await expect(page.getByRole("heading", { name: "Set up Aurora", level: 1 })).toBeVisible();
        const steps = page.getByRole("list", { name: "Setup steps" });
        for (const sid of ["signin", "placement", "key", "try", "email"]) await expect(steps.locator(`#setup-${sid}`)).toHaveCount(1);
        // One button per row; no form on the page.
        await expect(steps.locator("#setup-signin").getByRole("link", { name: "Review" })).toBeVisible();
        await expect(steps.locator("#setup-placement").getByRole("link", { name: "Change" })).toBeVisible();
        // (D-414, P5-REFINE-18 #14) No starter key: Try it is not offered until an agent can answer.
        await expect(steps.locator("#setup-try").getByRole("link", { name: "Open chat" })).toHaveCount(0);
        await expect(steps.locator("#setup-try")).toContainText("Comes after the AI key");
        await expect(steps.locator("#setup-email").getByRole("link", { name: "Set up" })).toBeVisible();
        await expect(page.getByText("2 of 5")).toBeVisible();
        await clean(page, width === 390);

        await steps.locator("#setup-key").getByRole("button", { name: "Add key" }).click();
        const dialog = page.getByRole("dialog", { name: "Add an organization AI key" });
        const form = dialog.getByRole("form", { name: "New LLM key" });
        await form.getByLabel("Name").fill("AIGW key");
        await form.getByLabel("Provider").selectOption("litellm");
        await form.getByLabel("API key", { exact: true }).fill("sk-test-key");
        await dialog.getByRole("button", { name: "Add key" }).click();
        // The checklist stays, with its toast; the health view is not shown (the try-it step is still open).
        // (P5-REFINE-18 #14) The toast names the key ("<name> added."), then the count.
        await expect(page.getByTestId("setup-toast")).toContainText("AIGW key added.");
        await expect(page.getByTestId("setup-toast")).toContainText("3 of 5 done.");
        await expect(page.getByRole("heading", { name: "Set up Aurora", level: 1 })).toBeVisible();
        await expect(steps.locator("#setup-key")).toHaveAttribute("data-done", "true");
        await expect(page.getByTestId("admin-health")).toHaveCount(0);
        await clean(page, width === 390);
        // The Health tab is always there.
        await page.getByRole("tab", { name: "Health" }).click();
        await expect(page.getByTestId("admin-health")).toBeVisible();
        expect(errors).toEqual([]);
      });

      test("the apps list leads to a review; Approve works without opening a section and returns with a toast", async ({ page }) => {
        const { errors } = await open(page, "/admin/apps", scheme);
        await expect(page.getByRole("heading", { name: "1 app is waiting for your approval." })).toBeVisible();
        const waiting = page.getByRole("region", { name: "1 app is waiting for your approval." });
        await expect(waiting).toContainText("Uploaded · unsigned · reaches tracker.acme.test");
        const aurora = page.getByRole("region", { name: "From Aurora" });
        await expect(aurora).toContainText("5 apps, approved with this release");
        await expect(aurora.getByRole("link", { name: "GitHub", exact: true })).toBeVisible();
        await expect(aurora.getByRole("link", { name: "Slack", exact: true })).toHaveCount(0);
        await aurora.getByRole("button", { name: "2 more" }).click();
        await expect(aurora.getByRole("link", { name: "Slack", exact: true })).toBeVisible();
        // No package ids and no amber caps.
        await expect(page.getByText("acme-tracker")).toHaveCount(0);
        await expect(page.getByText(/pending approval/i)).toHaveCount(0);
        await clean(page, width === 390);

        // The name is a real link to the review.
        await waiting.getByRole("link", { name: "Acme tracker", exact: true }).click();
        await expect(page).toHaveURL(new RegExp(`/admin/apps/packages/${UPLOADED}$`));
        await expect(page.getByRole("heading", { name: /Acme tracker/, level: 1 })).toBeVisible();
        // (P5-REFINE-18 #8) The summary and Approve come first; the sections sit in the closed Technical details, open inside.
        await expect(page.getByRole("heading", { name: "What this app does" })).toBeVisible();
        await page.getByText("Technical details", { exact: true }).click();
        await expect(page.getByRole("button", { name: /Egress per hook/ })).toHaveAttribute("aria-expanded", "true");
        await expect(page.getByText("your server's address")).toBeVisible();
        await expect(page.getByText("First install", { exact: true })).toBeVisible();
        const approve = page.getByRole("button", { name: "Approve" });
        await expect(approve).toBeEnabled();
        await clean(page, width === 390);

        await approve.click();
        await expect(page).toHaveURL(/\/admin\/apps\?tab=apps&approved=/);
        await expect(page.getByTestId("approved-toast")).toContainText("Acme tracker approved");
        await expect(page.getByRole("heading", { name: "No app is waiting." })).toBeVisible();
        expect(errors).toEqual([]);
      });
    });
  }
}
