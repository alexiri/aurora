import AxeBuilder from "@axe-core/playwright";
import { expect as baseExpect, test, type Page } from "@playwright/test";

import { mockApi } from "./support/mock-api";
import { overflow, titled } from "./support/open";

// The first request of a route compiles it in `next dev`; on a loaded host that outlasts the default 5 s.
const expect = baseExpect.configure({ timeout: 30_000 });

// P5-IA-3 (C-966, C-967): AI models and Agents at Administration and organization level. Old addresses landing on the
// right tabs are checked by ia-navigation.spec.ts.

async function axeClean(page: Page) {
  const { violations } = await new AxeBuilder({ page }).analyze();
  return violations
    .filter((v) => v.impact === "serious" || v.impact === "critical")
    .map((v) => `${v.id} (${v.impact}): ${v.nodes.map((n) => n.target.join(" ")).slice(0, 5).join(" | ")}`);
}

const json = (body: unknown, status = 200) => ({ status, contentType: "application/json", body: JSON.stringify(body) });
const set = <T>(value: T, source: "instance" | "org" | "default" = "default") => ({ value, source });

const effective = {
  llm: {
    posture: set("sanctioned_only", "instance"),
    endpoints: [
      {
        id: "gateway", provider: "litellm", display_name: "Users AI gateway",
        permitted: set(true, "instance"), key_scopes: set(["organization"], "instance"), allowed_models: set(["claude-sonnet"], "org"), origin_rule: null,
      },
    ],
  },
  egress: { posture: set("open"), open_lane: set("enabled"), org_allowlist: [], baseline_allowlist: [] },
  byo: { posture: set("owners_only", "instance"), source_cidrs: set(null), require_context_credentials: set(false) },
  agent_tokens: { max_age_days: set(90, "instance") },
  non_compliant_secrets: [
    { id: "s1", name: "old-key", workspace_id: null, reason: "mismatch" },
    { id: "s2", name: "lab-key", workspace_id: null, reason: "not_permitted" },
  ],
};
const governanceDoc = {
  llm: { posture: "sanctioned_only", endpoints: { gateway: { allowed: true, key_scopes: ["organization"], allowed_models: ["claude-sonnet"], origin: { allowed_organization_ids: [], allowed_workspace_ids: [], allowed_team_ids: [] } } } },
  egress: { posture: "open", open_lane: "enabled", allowlist: [] },
  byo: { posture: null, source_cidrs: [], require_context_credentials: false },
  agent_tokens: { max_age_days: null },
};

/** The shared mock plus an organization with an instance limit on its AI models and BYO, and a governance document that remembers PUTs. */
async function open(page: Page, path: string, scheme: "light" | "dark") {
  const errors: string[] = [];
  page.on("pageerror", (e) => errors.push(e.message));
  await mockApi(page);
  let doc: Record<string, unknown> = governanceDoc;
  let revision = 4;
  await page.route("**/v1/orgs/*/governance/effective", (r) => r.fulfill(json(effective)));
  await page.route("**/v1/orgs/*/agent-tokens**", (r) => r.fulfill(json({ items: [], next_cursor: null })));
  await page.route("**/v1/governance/instance", (r) =>
    r.fulfill(json({ llm: { allowed_key_scopes: [], minimum_posture: "sanctioned_only", require_key_origin: false, known_llm_hosts: [], endpoints: [] }, egress: { minimum_posture: "open", open_lane_minimum: "enabled", baseline_allowlist: [] }, byo: { enabled: true, minimum_posture: "owners_only", default_source_cidrs: [], require_context_credentials: false }, agent_tokens: { max_age_days: 90 } })),
  );
  await page.route(/\/v1\/orgs\/[^/]+\/governance$/, async (r) => {
    if (r.request().method() === "PUT") {
      const body = r.request().postDataJSON() as { document: Record<string, unknown> };
      doc = body.document;
      revision += 1;
    }
    return r.fulfill(json({ document: doc, revision }));
  });
  await page.emulateMedia({ colorScheme: scheme });
  await page.goto(path);
  await page.locator("h1, h2").first().waitFor();
  await page.waitForLoadState("networkidle");
  await expect.poll(() => page.locator("html").getAttribute("data-theme"), { timeout: 20_000 }).toBe(scheme);
  await titled(page);
  expect(errors, `${path} threw`).toEqual([]);
}

for (const scheme of ["light", "dark"] as const) {
  for (const width of [1280, 390]) {
    test.describe(`${scheme} theme, ${width} px`, () => {
      test.use({ viewport: { width, height: 900 } });

      test("an organization admin narrows the allowed models and the page keeps Administration's limit visible above", async ({ page }) => {
        await open(page, "/o/acme/ai-models", scheme);
        const above = page.getByRole("region", { name: "Set for you by an Aurora administrator" });
        await expect(above).toContainText("Which AI keys agents may use");
        await expect(above).toContainText("Only keys an administrator approved");
        await expect(above).toContainText("Set for you by an Aurora administrator");
        await expect(page.getByTestId("keys-notice")).toContainText("2 AI keys no longer follow these rules");
        await expect(page.getByRole("link", { name: /Accounts/ }).first()).toHaveAttribute("href", /kind=ai-key/);
        const models = page.getByLabel("Allowed models");
        await models.fill("claude-haiku");
        const put = page.waitForRequest((r) => r.method() === "PUT" && /\/governance$/.test(r.url()));
        // Save ends the page: at 390 px `next dev`'s own indicator sits over it and takes the pointer, so the click is dispatched.
        await page.getByRole("button", { name: "Save" }).dispatchEvent("click");
        expect((await put).postDataJSON().document.llm.endpoints.gateway.allowed_models).toEqual(["claude-haiku"]);
        await expect(page.getByText(/Saved\./)).toBeVisible();
        await expect(above).toContainText("Only keys an administrator approved");
        await expect(page.getByRole("tab")).toHaveCount(0);
        expect(await overflow(page)).toBeLessThanOrEqual(0);
        expect(await axeClean(page)).toEqual([]);
      });

      test("the organization's Agents page: the list, then Bring your own under Administration's limits (with Previews) and Skills", async ({ page }) => {
        await open(page, "/o/acme/agents", scheme);
        await expect(page.getByRole("tab")).toHaveText(["Agents", "Bring your own", "Skills"]);
        await page.getByRole("tab", { name: "Bring your own" }).click();
        await expect(page).toHaveURL(/\/o\/acme\/agents\?tab=byo$/);
        const above = page.getByRole("region", { name: "Set for you by an Aurora administrator" });
        await expect(above).toContainText("Only workspace owners may run agents outside Aurora");
        await expect(above).toContainText("Up to 90 days");
        await expect(page.getByLabel("Maximum token age (days)")).toBeVisible();
        expect(await overflow(page)).toBeLessThanOrEqual(0);
        expect(await axeClean(page)).toEqual([]);
        await expect(page.getByRole("region", { name: "Port previews" })).toBeVisible();
        expect(await axeClean(page)).toEqual([]);
      });

      test("Administration › Agents has four tabs, each a coherent page", async ({ page }) => {
        await open(page, "/admin/agents", scheme);
        await expect(page.getByRole("tab")).toHaveText(["Defaults", "BYO", "Computers", "Skills"]);
        // One heading for the page: the tabs' sections do not repeat it as another title.
        await expect(page.getByRole("heading", { name: "Agents", level: 1 })).toHaveCount(1);
        // The agents section's groups and its runners table are one run on the tab, not a page of their own.
        // (P5-POLISH-2) Settings groups are SettingsCards: a section titled by its heading, no longer a named group.
        await expect(page.getByRole("heading", { name: "Defaults for new agents" })).toBeVisible();
        await expect(page.getByRole("heading", { name: "Runners and sandbox sizes" })).toBeVisible();
        expect(await overflow(page)).toBeLessThanOrEqual(0);
        expect(await axeClean(page)).toEqual([]);
        for (const tab of ["BYO", "Computers", "Skills"]) {
          await page.getByRole("tab", { name: tab }).click();
          await expect(page.getByRole("tab", { name: tab })).toHaveAttribute("aria-selected", "true");
          expect(await overflow(page)).toBeLessThanOrEqual(0);
          expect(await axeClean(page)).toEqual([]);
        }
      });

      test("Administration › AI models has Endpoints and Rules, with the organizations beneath the rules", async ({ page }) => {
        await open(page, "/admin/ai-models", scheme);
        await expect(page.getByRole("tab")).toHaveText(["Endpoints", "Rules"]);
        await expect(page.getByRole("heading", { name: "AI models", level: 1 })).toHaveCount(1);
        expect(await axeClean(page)).toEqual([]);
        await page.getByRole("tab", { name: "Rules" }).click();
        await expect(page.getByRole("heading", { name: "AI keys" })).toBeVisible();
        await expect(page.getByRole("region", { name: "Where organizations stand" })).toBeVisible();
        expect(await overflow(page)).toBeLessThanOrEqual(0);
        expect(await axeClean(page)).toEqual([]);
      });
    });
  }
}
