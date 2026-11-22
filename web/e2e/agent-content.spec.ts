import AxeBuilder from "@axe-core/playwright";
import { expect, test, type Page } from "@playwright/test";

import { mockApi } from "./support/mock-api";
import { mockWithTerminal } from "./support/terminal";
import { CANVAS, overflow, titled, type Scheme } from "./support/open";

// P5-APPSKILL-3 (C-1136, C-1139; 14 §Plugins, 24 §9): the "For agents" section of a package and of its pending-version
// review — the skills and their files, scripts and change marks, the one line above Approve, a file opened as plain
// text, hostile text as inert data, both themes, no horizontal page scroll at 390 px (a 64 KiB file of 400-character
// lines scrolls inside its viewer), no serious axe violation — and an agent's Skills page with the "From apps" group.
// The API is mocked.

const PKG = "00000000-0000-4000-8000-0000000009a0";
const LONG = `${"a".repeat(400)}\n`.repeat(160); // 64 160 bytes
const HOSTILE = '# API\n<img src=x onerror="window.__pwned=1">\n[click](javascript:window.__pwned=2)\n';

const plugin = {
  id: PKG,
  package_id: "fixture-app-skills",
  version: "1.1.0",
  contract: 1,
  publisher: "fixtures",
  digest: "d".repeat(64),
  signer: { kind: "trusted_key", key_id: "k1", name: "Fixture key" },
  source: "upload",
  origin: "uploaded",
  summary: null,
  reaches: [],
  reaches_more: 0,
  source_id: null,
  update_available: false,
  status: "pending_approval",
  status_reason: null,
  violations: [],
  apps: [{ app_id: "skills-demo", name: "Skills demo" }],
  permissions: { hooks: [], egress: {}, context: [] },
  host_claims: [],
  widenings: [],
  impact: null,
  approved_by: null,
  approved_at: null,
  created_at: "2026-11-18T00:00:00Z",
  agent: [
    {
      app_id: "skills-demo",
      summary: "Things, lists and creating them.",
      instructions: "Use numeric ids.\nNever retry a create.",
      skills: [
        {
          name: "skills-demo-api",
          description: "REST conventions",
          size: HOSTILE.length,
          files: [
            { path: "SKILL.md", size: HOSTILE.length, sha256: "a".repeat(64), executable: false },
            { path: "scripts/list.sh", size: 120, sha256: "e".repeat(64), executable: true },
          ],
        },
        {
          name: "skills-demo-steps",
          description: "Create a thing",
          size: LONG.length,
          files: [{ path: "SKILL.md", size: LONG.length, sha256: "b".repeat(64), executable: false }],
        },
      ],
      operations: null,
      changed: {
        summary: false,
        instructions: true,
        operations: false,
        skills: [
          { name: "skills-demo-steps", change: "added", files: [{ path: "SKILL.md", change: "added" }] },
          {
            name: "skills-demo-api",
            change: "changed",
            files: [
              { path: "SKILL.md", change: "changed" },
              { path: "scripts/list.sh", change: "changed" },
              { path: "scripts/list.sh", change: "mode" },
            ],
          },
          { name: "skills-demo-old", change: "removed", files: [{ path: "SKILL.md", change: "removed" }] },
        ],
      },
    },
  ],
};

async function open(page: Page, scheme: Scheme) {
  const errors: string[] = [];
  page.on("pageerror", (e) => errors.push(e.message));
  await mockApi(page);
  await page.route(`**/v1/admin/plugins/${PKG}`, (route) =>
    route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify(plugin) }),
  );
  await page.route(`**/v1/admin/plugins/${PKG}/apps/skills-demo/skills/*/file`, (route) => {
    const steps = route.request().url().includes("/skills-demo-steps/");
    return route.fulfill({
      status: 200,
      contentType: "text/plain; charset=utf-8",
      body: steps ? LONG : HOSTILE,
    });
  });
  await page.emulateMedia({ colorScheme: scheme });
  await page.goto(`/admin/apps/packages/${PKG}`);
  await page.getByRole("heading", { name: "For agents", level: 2 }).waitFor();
  await page.waitForLoadState("networkidle");
  await expect.poll(() => page.locator("html").getAttribute("data-theme"), { timeout: 20_000 }).toBe(scheme);
  await titled(page);
  expect(errors).toEqual([]);
}

for (const width of [1280, 390]) {
  test.describe(`For agents on a package (${width} px)`, () => {
    test.use({ viewport: { width, height: 900 } });
    for (const scheme of ["light", "dark"] as const) {
      test(`files, change marks, plain-text viewer, no horizontal scroll, accessible (${scheme})`, async ({ page }) => {
        await open(page, scheme);
        expect(await page.evaluate(() => getComputedStyle(document.body).backgroundColor)).toBe(CANVAS[scheme]);
        // (P5-POLISH-10) The review's sections are open from the start.
        await expect(page.getByRole("button", { name: /What agents get/ })).toHaveAttribute("aria-expanded", "true");
        const app = page.getByTestId("agent-content-skills-demo");
        await expect(app.getByRole("heading", { name: "Skills", exact: true })).toBeVisible();
        await expect(app.getByText("skills-demo-old", { exact: true })).toBeVisible();
        // Scripts and change marks (C-1139, C-1136's changed + mode pair).
        const api = app.getByTestId("agent-skill-skills-demo-api");
        await expect(api.getByText("Script", { exact: true }).first()).toBeVisible();
        await expect(api.getByText("Now a script")).toBeVisible();
        await expect(page.getByTestId("skills-changed-line")).toHaveText(
          "Skills for agents changed: 1 added, 1 changed, 1 removed, 1 script",
        );
        await expect(page.getByText(/Agents that use this app get these skills/)).toBeVisible();

        // A hostile file is text: no element, no link, nothing executed; its first line is visible.
        await app.getByRole("button", { name: "skills-demo-api/SKILL.md" }).click();
        const viewer = page.getByRole("region", { name: "File skills-demo-api/SKILL.md" });
        await expect(viewer).toContainText("# API");
        expect(await viewer.locator("*").count()).toBe(0);
        expect(await page.locator("a[href^='javascript:']").count()).toBe(0);
        expect(await page.evaluate(() => (window as unknown as { __pwned?: number }).__pwned)).toBeUndefined();

        // 64 KiB of 400-character lines: the viewer scrolls, the page does not.
        await app.getByRole("button", { name: "skills-demo-steps/SKILL.md" }).click();
        const big = page.getByRole("region", { name: "File skills-demo-steps/SKILL.md" });
        await expect(big).toBeVisible();
        expect(await big.evaluate((el) => el.scrollWidth > el.clientWidth)).toBe(true);
        expect(await overflow(page), "page overflows horizontally").toBeLessThanOrEqual(0);

        const { violations } = await new AxeBuilder({ page }).analyze();
        const bad = violations
          .filter((v) => v.impact === "serious" || v.impact === "critical")
          .map((v) => `${v.id} (${v.impact}): ${v.nodes.map((n) => n.target.join(" ")).slice(0, 5).join(" | ")}`);
        expect(bad).toEqual([]);
      });
    }
  });
}

test.describe("the review", () => {
  test.use({ viewport: { width: 1280, height: 900 } });
  test("For agents is open, the skills line links to it, and approval is enabled with no second confirmation", async ({ page }) => {
    await open(page, "light");
    const approve = page.getByRole("button", { name: "Approve" });
    await expect(page.getByRole("button", { name: /What agents get/ })).toHaveAttribute("aria-expanded", "true");
    await expect(page.getByTestId("skills-changed-line").getByRole("link", { name: "Skills for agents changed" })).toHaveAttribute("href", "#for-agents");
    await expect(approve).toBeEnabled();
  });
});

// The agent's Skills page: app skills under "From apps", read-only, a shadowed one naming its winner.
const APP = { app_id: "skills-demo", name: "Skills demo", version: "1.1.0" };
const appBody = (name: string, over: Record<string, unknown> = {}) => ({
  id: null, org_id: null, agent_id: null, workspace_id: null, created_at: null, updated_at: null,
  tier: "app", scope: "app", name, description: `${name} description`, body: `# ${name}\n<b>text</b>`,
  enabled: true, files: [], total_size: 20, overridden_in: null, ...over,
});

for (const width of [390, 768, 1280]) {
  test.describe(`agent Skills page with app skills (${width} px)`, () => {
    test.use({ viewport: { width, height: 900 } });
    for (const scheme of ["light", "dark"] as const) {
      test(`From apps is read-only, no horizontal scroll, accessible (${scheme})`, async ({ page }) => {
        const errors: string[] = [];
        page.on("pageerror", (e) => errors.push(e.message));
        await mockWithTerminal(page, { output: new Uint8Array(), resizes: [] });
        await page.route("**/v1/agents/*/skills/effective", (route) =>
          route.fulfill({
            status: 200,
            contentType: "application/json",
            body: JSON.stringify({
              items: [
                { skill: appBody("skills-demo-api", { files: [{ path: "scripts/list.sh", size: 12, executable: true }] }), tier: "app", enabled: true, shadows: [], app: APP },
              ],
              shadowed: [
                { skill: appBody("triage"), tier: "app", enabled: true, shadowed_by: { id: "5b1c0a54-0000-4000-8000-000000000001", tier: "agent", app_id: null }, app: APP },
              ],
            }),
          }),
        );
        await page.route("**/v1/agents/*/skills", (route) =>
          route.request().method() === "GET"
            ? route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify({ items: [], next_cursor: null }) })
            : route.fallback(),
        );
        await page.emulateMedia({ colorScheme: scheme });
        await page.goto("/o/acme/w/main/agents/daneel?tab=skills");
        const group = page.getByRole("list", { name: "Skills from apps" });
        await expect(group).toBeVisible();
        await expect.poll(() => page.locator("html").getAttribute("data-theme"), { timeout: 20_000 }).toBe(scheme);
        await titled(page);
        expect(await page.evaluate(() => getComputedStyle(document.body).backgroundColor)).toBe(CANVAS[scheme]);
        const row = group.getByTestId("app-skill-skills-demo-api");
        await expect(row).toContainText("From the Skills demo app · 1.1.0");
        await expect(row.getByText("Script", { exact: true })).toBeHidden(); // inside the closed fold
        await row.getByText("Read the skill").click();
        await expect(row.getByText("Script", { exact: true })).toBeVisible();
        await expect(row.getByRole("region", { name: "Instructions of skills-demo-api" })).toContainText("<b>text</b>");
        // Read-only: nothing to edit, switch off or delete in the row.
        expect(await row.getByRole("button").count()).toBe(0);
        expect(await row.getByRole("switch").count()).toBe(0);
        await expect(page.getByRole("list", { name: "Skills not applied" })).toContainText("Shadowed by the agent skill");
        expect(await overflow(page), "page overflows horizontally").toBeLessThanOrEqual(0);
        const { violations } = await new AxeBuilder({ page }).analyze();
        const bad = violations
          .filter((v) => v.impact === "serious" || v.impact === "critical")
          .map((v) => `${v.id} (${v.impact}): ${v.nodes.map((n) => n.target.join(" ")).slice(0, 5).join(" | ")}`);
        expect(bad).toEqual([]);
        expect(errors).toEqual([]);
      });
    }
  });
}
