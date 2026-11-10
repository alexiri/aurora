import AxeBuilder from "@axe-core/playwright";
import { expect, test } from "@playwright/test";

import { openLibraryScreen, LIBRARY_SCREENS } from "./support/skill-libraries";
import { CANVAS, overflow } from "./support/open";

// P4-GSL (docs/14 §Skills, §Skill libraries; docs/18 §Git skill library, Dashboard): the libraries page, the
// connect dialog, Add from library (list and full preview) and the update review are themed in both
// themes, never scroll horizontally at 390 px, have no serious axe violations, and render hostile
// repository text (markup, a javascript: link) as inert data. The API is mocked.

for (const width of [1280, 390]) {
  test.describe(`skill library screens (${width} px)`, () => {
    test.use({ viewport: { width, height: 900 } });
    for (const scheme of ["light", "dark"] as const) {
      for (const s of LIBRARY_SCREENS) {
        test(`${s}: themed, no horizontal page scroll, accessible (${scheme})`, async ({ page }) => {
          await openLibraryScreen(page, s, scheme);
          expect(await page.evaluate(() => getComputedStyle(document.body).backgroundColor)).toBe(CANVAS[scheme]);
          expect(await overflow(page), `${s} overflows horizontally`).toBeLessThanOrEqual(0);
          if (s !== "libraries") {
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
  test("a markup or link payload in a description or diff is inert", async ({ page }) => {
    await openLibraryScreen(page, "review", "light");
    const dlg = page.getByRole("dialog");
    // the payload is visible as text...
    await expect(dlg.getByText("<img src=x onerror=alert(1)>", { exact: false }).first()).toBeVisible();
    // ...and never became an element or a link
    expect(await dlg.locator("img").count()).toBe(0);
    expect(await dlg.locator("a[href^='javascript:']").count()).toBe(0);
    expect(await page.locator("a[href^='javascript:']").count()).toBe(0);
    // the whole diff is shown: old and new lines of the body and the file
    await expect(dlg.getByText("Check the tests first.")).toBeVisible();
    await expect(dlg.getByText("- docs", { exact: false })).toBeVisible();
  });
});
