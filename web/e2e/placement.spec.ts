import AxeBuilder from "@axe-core/playwright";
import { expect, test } from "@playwright/test";

import { openRoute, overflow } from "./support/open";

// P4-ONB-1 (D-140): Admin › Sign-up & placement in both themes at 1280 and 390 px: no serious or
// critical axe violation, no horizontal scroll, and the live name preview works.

for (const scheme of ["light", "dark"] as const) {
  for (const width of [1280, 390]) {
    test.describe(`${scheme} theme, ${width} px`, () => {
      test.use({ viewport: { width, height: 900 } });

      test("sign-up & placement has no serious or critical violations and no horizontal scroll", async ({ page }) => {
        await openRoute(page, "/admin/placement", scheme);
        await expect(page.getByRole("heading", { name: "Sign-up & placement", level: 1 })).toBeVisible();
        await expect(page.getByTestId("placement-preview")).toContainText("bobj");
        expect(await overflow(page)).toBeLessThanOrEqual(0);
        const { violations } = await new AxeBuilder({ page }).analyze();
        const bad = violations
          .filter((v) => v.impact === "serious" || v.impact === "critical")
          .map((v) => `${v.id} (${v.impact}): ${v.nodes.map((n) => n.target.join(" ")).slice(0, 5).join(" | ")}`);
        expect(bad).toEqual([]);
      });
    });
  }
}
