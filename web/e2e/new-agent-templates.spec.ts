import AxeBuilder from "@axe-core/playwright";
import { expect, test } from "@playwright/test";

import { openGallery } from "./support/new-agent-templates";
import { CANVAS, overflow } from "./support/open";

// P6-TPL-4 Part B1 (C-1116, D-458): **More templates…** in the New agent dialog opens the gallery of the
// workspace's templates; choosing a card opens its details first — the README rendered as inert data, what it
// needs, what it runs on its own, its helper files by name and the version — before **Use this template**. A
// card whose needs are not met says so and cannot be used. The gallery is themed in both themes, never scrolls
// horizontally at 390 px, and has no serious axe violations. The API is mocked.

for (const width of [1280, 390]) {
  test.describe(`template gallery (${width} px)`, () => {
    test.use({ viewport: { width, height: 900 } });
    for (const scheme of ["light", "dark"] as const) {
      test(`gallery: themed, no horizontal page scroll, accessible (${scheme})`, async ({ page }) => {
        await openGallery(page, scheme);
        expect(await page.evaluate(() => getComputedStyle(document.body).backgroundColor)).toBe(CANVAS[scheme]);
        expect(await overflow(page), "gallery overflows horizontally").toBeLessThanOrEqual(0);
        const dlg = page.getByRole("dialog", { name: "More templates" });
        // Dialogs scroll inside themselves; nothing in them may be wider than the dialog.
        expect(await dlg.evaluate((el) => el.scrollWidth - el.clientWidth), "gallery dialog overflows").toBeLessThanOrEqual(0);
        const { violations } = await new AxeBuilder({ page }).analyze();
        const bad = violations
          .filter((v) => v.impact === "serious" || v.impact === "critical")
          .map((v) => `${v.id} (${v.impact}): ${v.nodes.map((n) => n.target.join(" ")).slice(0, 5).join(" | ")}`);
        expect(bad).toEqual([]);
      });
    }

    test("the card → details journey: what it needs, runs on its own, helper files, inert README", async ({ page }) => {
      await openGallery(page, "light");
      const dlg = page.getByRole("dialog", { name: "More templates" });
      // The cards: name, version, what the template runs on its own, and the not-ready sentences.
      await expect(dlg.getByText("Schedule manager")).toBeVisible();
      await expect(dlg.getByText("Version 1.0.0")).toBeVisible();
      await expect(dlg.getByText("Version 1.1.0")).toBeVisible();
      await expect(dlg.getByText("Schedule Morning triage")).toBeVisible();
      await expect(dlg.getByText("Webhook Alerts, created paused")).toBeVisible();
      await expect(dlg.getByText("Needs GitHub, which isn't available here.")).toBeVisible();
      // Choosing a card opens its details first (D-458).
      await dlg.getByRole("button", { name: /Schedule manager/ }).click();
      const details = page.getByTestId("template-details");
      await expect(details.getByRole("heading", { level: 3, name: "Schedule manager" }).first()).toBeVisible();
      const readme = page.getByTestId("template-readme");
      await expect(readme.getByText("<img src=x onerror=alert(1)>")).toBeVisible();
      expect(await readme.locator("img").count()).toBe(0);
      expect(await readme.locator("a[href^='javascript:']").count()).toBe(0);
      await expect(readme.getByText("remote picture")).toBeVisible();
      await expect(readme.getByText("docs")).toBeVisible();
      await expect(readme.getByRole("heading", { level: 3, name: "Schedule manager" })).toBeVisible();
      await expect(details.getByText("GitHub — required")).toBeVisible();
      await expect(details.getByText("Schedule Morning triage")).toBeVisible();
      await expect(details.getByText("triage.py")).toBeVisible();
      await expect(details.getByText("Version 1.0.0")).toBeVisible();
      // Back returns to the gallery (the dialog header's arrow is the first Back control).
      await page.getByRole("button", { name: "Back", exact: true }).first().click();
      await expect(page.getByRole("dialog", { name: "More templates" }).getByText("Schedule manager")).toBeVisible();
    });

    test("a card whose needs are not met opens details that say so and cannot be used (D-470)", async ({ page }) => {
      await openGallery(page, "light");
      const dlg = page.getByRole("dialog", { name: "More templates" });
      await dlg.getByRole("button", { name: /Incident scribe/ }).click();
      const details = page.getByTestId("template-details");
      // The details header is the first level-3 heading; the README's own `# …` heading is inside the readme.
      await expect(details.getByRole("heading", { level: 3, name: "Incident scribe" }).first()).toBeVisible();
      await expect(details.getByText("Needs GitHub, which isn't available here.")).toBeVisible();
      await expect(details.getByRole("button", { name: "Use this template" })).toBeDisabled();
    });
  });
}
