import AxeBuilder from "@axe-core/playwright";
import { expect, test } from "@playwright/test";

import { ADDON_SCREENS, mockAddonSources, openAddonScreen } from "./support/addon-sources";
import { CANVAS, overflow } from "./support/open";

// P5-ASRC (docs/14 §Admin, docs/18): Admin › Addon sources — the list, the add/review dialog with the
// signing-key fingerprint, the key re-confirm, the schedule and the addon details — is themed in both
// themes, never scrolls horizontally at 390 px, has no serious axe violation, and renders hostile
// repository text (markup, a javascript: link) as inert data. The API is mocked.

for (const width of [1280, 390]) {
  test.describe(`addon source screens (${width} px)`, () => {
    test.use({ viewport: { width, height: 900 } });
    for (const scheme of ["light", "dark"] as const) {
      for (const s of ADDON_SCREENS) {
        test(`${s}: themed, no horizontal page scroll, accessible (${scheme})`, async ({ page }) => {
          await openAddonScreen(page, s, scheme);
          expect(await page.evaluate(() => getComputedStyle(document.body).backgroundColor)).toBe(CANVAS[scheme]);
          expect(await overflow(page), `${s} overflows horizontally`).toBeLessThanOrEqual(0);
          if (s !== "list") {
            // Dialogs scroll inside themselves; nothing in them may be wider than the dialog.
            const dlg = page.getByRole("dialog");
            expect(await dlg.evaluate((el) => el.scrollWidth - el.clientWidth), `${s} dialog overflows`).toBeLessThanOrEqual(0);
          }
          const { violations } = await new AxeBuilder({ page }).analyze();
          const bad = violations
            .filter((v) => v.impact === "serious" || v.impact === "critical")
            .map((v) => `${v.id} (${v.impact}): ${v.nodes.map((n) => n.target.join(" ")).slice(0, 5).join(" | ")}`);
          expect(bad).toEqual([]);
        });
      }
    }
  });
}

test.describe("repository text is data", () => {
  test.use({ viewport: { width: 1280, height: 900 } });
  test("a hostile description is text, never markup or a link", async ({ page }) => {
    await openAddonScreen(page, "details", "light");
    const dlg = page.getByRole("dialog");
    await expect(dlg.getByText(/onerror=alert\(1\)/)).toBeVisible();
    expect(await dlg.locator("img").count()).toBe(0);
    expect(await dlg.locator("a[href^='javascript:']").count()).toBe(0);
  });
});

test.describe("the plugins list carries provenance and the check button", () => {
  test.use({ viewport: { width: 1280, height: 900 } });
  test("shows the source, an update badge and checks every source", async ({ page }) => {
    await mockAddonSources(page);
    await page.route("**/v1/admin/plugins?*", (route) =>
      route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          items: [
            {
              id: "00000000-0000-0000-0000-0000000008a0", package_id: "indigo_sso", version: "1.0.0", contract: 1, publisher: "cern",
              digest: "d", signer: { kind: "trusted_key", name: "CERN addons signing key" }, source: "addon",
              source_id: "00000000-0000-0000-0000-000000000320", source_name: "CERN addons", update_available: true,
              status: "active", status_reason: null, approved_at: null, created_at: new Date().toISOString(),
              // Required by openapi `PluginPackage` (C-1032): the list renders origin, reaches and the app name.
              origin: "addon", reaches: [], reaches_more: 0, violations: [], apps: [], permissions: {}, host_claims: [], widenings: [],
            },
          ],
          next_cursor: null,
        }),
      }),
    );
    await page.route("**/v1/admin/addon-sources/check-all", (route) =>
      route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ items: [{ source_id: "00000000-0000-0000-0000-000000000320", status: "up_to_date", ingested: [], failed_versions: [], check_failed_reason: null }] }),
      }),
    );
    await page.goto("/admin/plugins");
    // (P5-POLISH-10) The row's origin line names the addon source as text (the sources are linked from the update notice).
    await expect(page.getByText("CERN addons", { exact: true })).toBeVisible();
    await expect(page.getByRole("link", { name: "See the addon sources" })).toBeVisible();
    await expect(page.getByText("Update available").first()).toBeVisible();
    await page.getByRole("button", { name: "Check for updates" }).click();
    await expect(page.getByLabel("Update check results")).toBeVisible();
  });
});
