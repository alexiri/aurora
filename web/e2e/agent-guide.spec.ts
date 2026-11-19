import AxeBuilder from "@axe-core/playwright";
import { expect, test, type Page } from "@playwright/test";

import { mockApi } from "./support/mock-api";
import { CANVAS, overflow, titled, type Scheme } from "./support/open";

// P5-GUIDE-3 (C-648; 14 §Plugins, 24 §9): the Agent guide section of a package and of its pending-version review —
// the files, the change marks, a file opened as plain text, hostile text as inert data, themed in both themes, no
// horizontal page scroll at 390 px (a 64 KiB file of 400-character lines scrolls inside its viewer), no serious axe
// violation. The API is mocked.

const PKG = "00000000-0000-4000-8000-0000000009a0";
const LONG = `${"a".repeat(400)}\n`.repeat(160); // 64 160 bytes
const HOSTILE = '# API\n<img src=x onerror="window.__pwned=1">\n[click](javascript:window.__pwned=2)\n';

const plugin = {
  id: PKG,
  package_id: "fixture-guide",
  version: "1.1.0",
  contract: 1,
  publisher: "fixtures",
  digest: "d".repeat(64),
  signer: { kind: "trusted_key", key_id: "k1", name: "Fixture key" },
  source: "upload",
  source_id: null,
  update_available: false,
  status: "pending_approval",
  status_reason: null,
  violations: [],
  apps: [{ app_id: "guide-demo", name: "Guide demo" }],
  permissions: { hooks: [], egress: {}, context: [] },
  host_claims: [],
  widenings: [],
  impact: null,
  approved_by: null,
  approved_at: null,
  created_at: "2026-11-18T00:00:00Z",
  agent_guides: [
    {
      app_id: "guide-demo",
      summary: "Things, lists and creating them.",
      instructions: "Use numeric ids.\nNever retry a create.",
      files: [
        { file: "api.md", title: "REST conventions", kind: "reference", size: HOSTILE.length, sha256: "a".repeat(64) },
        { file: "steps.md", title: "Create a thing", kind: "recipe", size: LONG.length, sha256: "b".repeat(64) },
      ],
      operations: null,
      changed: {
        summary: false,
        instructions: true,
        operations: false,
        files: [
          { file: "steps.md", change: "added" },
          { file: "api.md", change: "changed" },
          { file: "old.md", change: "removed" },
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
  await page.route(`**/v1/admin/plugins/${PKG}/agent-guides/guide-demo/*`, (route) => {
    const file = route.request().url().split("/").pop();
    return route.fulfill({
      status: 200,
      contentType: "text/plain; charset=utf-8",
      body: file === "steps.md" ? LONG : HOSTILE,
    });
  });
  await page.emulateMedia({ colorScheme: scheme });
  await page.goto(`/admin/apps/packages/${PKG}`);
  await page.getByRole("heading", { name: "Agent guide", level: 2 }).waitFor();
  await page.waitForLoadState("networkidle");
  await expect.poll(() => page.locator("html").getAttribute("data-theme"), { timeout: 20_000 }).toBe(scheme);
  await titled(page);
  expect(errors).toEqual([]);
}

for (const width of [1280, 390]) {
  test.describe(`agent guide of a package (${width} px)`, () => {
    test.use({ viewport: { width, height: 900 } });
    for (const scheme of ["light", "dark"] as const) {
      test(`files, change marks, plain-text viewer, no horizontal scroll, accessible (${scheme})`, async ({ page }) => {
        await open(page, scheme);
        expect(await page.evaluate(() => getComputedStyle(document.body).backgroundColor)).toBe(CANVAS[scheme]);
        await page.getByRole("button", { name: /What agents can read/ }).click();
        const app = page.getByTestId("agent-guide-guide-demo");
        await expect(app.getByRole("heading", { name: "Recipes" })).toBeVisible();
        await expect(app.getByRole("heading", { name: "Reference files" })).toBeVisible();
        await expect(app.getByText("old.md")).toBeVisible();
        await expect(page.getByText("Agents that use this app can read this text. It is written by the publisher.")).toBeVisible();

        // A hostile file is text: no element, no link, nothing executed; its first line is visible.
        await app.getByRole("button", { name: "api.md" }).click();
        const viewer = page.getByRole("region", { name: "File api.md" });
        await expect(viewer).toContainText("# API");
        expect(await viewer.locator("*").count()).toBe(0);
        expect(await page.locator("a[href^='javascript:']").count()).toBe(0);
        expect(await page.evaluate(() => (window as unknown as { __pwned?: number }).__pwned)).toBeUndefined();

        // 64 KiB of 400-character lines: the viewer scrolls, the page does not.
        await app.getByRole("button", { name: "steps.md" }).click();
        const big = page.getByRole("region", { name: "File steps.md" });
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
  test("the agent guide section is open and approval is enabled, with no second confirmation", async ({ page }) => {
    await open(page, "light");
    const approve = page.getByRole("button", { name: "Approve" });
    await expect(page.getByRole("button", { name: /What agents can read/ })).toHaveAttribute("aria-expanded", "true");
    await expect(approve).toBeEnabled();
  });
});
