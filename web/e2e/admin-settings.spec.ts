import AxeBuilder from "@axe-core/playwright";
import { expect, test } from "@playwright/test";

import { openRoute, overflow } from "./support/open";

// SET-1 (D-236): Admin › Settings and the generic section form in both themes at 1280 and 390 px:
// no serious or critical axe violation, no horizontal scroll, locked settings read-only.

async function axeClean(page: import("@playwright/test").Page) {
  const { violations } = await new AxeBuilder({ page }).analyze();
  return violations
    .filter((v) => v.impact === "serious" || v.impact === "critical")
    .map((v) => `${v.id} (${v.impact}): ${v.nodes.map((n) => n.target.join(" ")).slice(0, 5).join(" | ")}`);
}

for (const scheme of ["light", "dark"] as const) {
  for (const width of [1280, 390]) {
    test.describe(`${scheme} theme, ${width} px`, () => {
      test.use({ viewport: { width, height: 900 } });

      test("the settings index lists the sections and the locked key", async ({ page }) => {
        await openRoute(page, "/admin/settings", scheme);
        await expect(page.getByRole("heading", { name: "Settings", level: 1 })).toBeVisible();
        await expect(page.getByRole("link", { name: "Governance", exact: true })).toBeVisible();
        await expect(page.getByText(/1 setting is configured in Helm and cannot be changed here/)).toBeVisible();
        expect(await overflow(page)).toBeLessThanOrEqual(0);
        expect(await axeClean(page)).toEqual([]);
      });

      test("a section shows a locked setting read-only and the editable ones", async ({ page }) => {
        await openRoute(page, "/admin/settings/governance", scheme);
        await expect(page.getByRole("heading", { name: "Governance", level: 1 })).toBeVisible();
        await expect(page.getByTestId("governance.egress.minimum_posture-locked-value")).toHaveText("restricted");
        await expect(page.getByText("config.governance.egress.minimum_posture").first()).toBeVisible();
        await expect(page.locator("[id='governance.egress.minimum_posture-c']")).toHaveCount(0);
        await expect(page.locator("[id='governance.llm.require_key_origin-c']")).toBeChecked();
        await expect(page.getByText("Managed by Helm")).toBeVisible();
        expect(await overflow(page)).toBeLessThanOrEqual(0);
        expect(await axeClean(page)).toEqual([]);
      });
    });
  }
}
