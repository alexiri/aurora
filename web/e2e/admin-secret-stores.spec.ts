import AxeBuilder from "@axe-core/playwright";
import { expect, test } from "@playwright/test";

import { openRoute, overflow } from "./support/open";

// FIX-124: Admin › Secret stores in both themes at 1280 and 390 px: no serious or critical axe
// violation, no horizontal scroll, a Helm store read-only, a dashboard store editable.

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

      test("the stores page shows Helm and dashboard stores", async ({ page }) => {
        await openRoute(page, "/admin/secret-stores", scheme);
        await expect(page).toHaveURL(/\/admin\/platform\?tab=secret-stores$/);
        await expect(page.getByRole("heading", { name: "Platform", level: 1 })).toBeVisible();
        const list = page.getByRole("list", { name: "Secret stores" });
        await expect(list.getByText("Set in Helm").first()).toBeVisible();
        await expect(list.getByRole("button", { name: "Edit team-vault" })).toBeVisible();
        await expect(list.getByRole("button", { name: "Edit vault" })).toHaveCount(0);
        await expect(page.getByText("Not answering")).toBeVisible();
        expect(await overflow(page)).toBeLessThanOrEqual(0);
        expect(await axeClean(page)).toEqual([]);
      });

      test("the add dialog fits and passes axe", async ({ page }) => {
        await openRoute(page, "/admin/secret-stores", scheme);
        await page.getByRole("button", { name: "Add a Vault store" }).click();
        await expect(page.getByRole("dialog")).toBeVisible();
        expect(await axeClean(page)).toEqual([]);
        expect(await overflow(page)).toBeLessThanOrEqual(0);
      });
    });
  }
}
