import AxeBuilder from "@axe-core/playwright";
import { expect, test } from "@playwright/test";

import { openTemplateScreen, TEMPLATE_SCREENS } from "./support/agent-templates";
import { CANVAS, overflow } from "./support/open";

// P6-TPL-4 (C-1114; docs/14 §Agent templates): the Administration › What agents may do › Agent templates tab —
// the template repositories with their two-step Add dialog (Check reads, nothing is stored until the add), Check
// now, Edit and Remove, and the templates themselves with their review chips — is themed in both themes, never
// scrolls horizontally at 390 px, has no serious axe violations, and its journeys write exactly what the screen
// showed. The API is mocked.

for (const width of [1280, 390]) {
  test.describe(`agent template screens (${width} px)`, () => {
    test.use({ viewport: { width, height: 900 } });
    for (const scheme of ["light", "dark"] as const) {
      for (const s of TEMPLATE_SCREENS) {
        test(`${s}: themed, no horizontal page scroll, accessible (${scheme})`, async ({ page }) => {
          await openTemplateScreen(page, s, scheme);
          expect(await page.evaluate(() => getComputedStyle(document.body).backgroundColor)).toBe(CANVAS[scheme]);
          expect(await overflow(page), `${s} overflows horizontally`).toBeLessThanOrEqual(0);
          if (s === "add") {
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

for (const width of [1280, 390]) {
  test.describe(`agent template journeys (${width} px)`, () => {
    test.use({ viewport: { width, height: 900 } });

    test("add a repository: the address → check → what Aurora found → add", async ({ page }) => {
      const mock = await openTemplateScreen(page, "tab", "light");
      await expect(page.getByRole("heading", { name: "Template repositories" })).toBeVisible();
      await expect(page.getByRole("heading", { name: "Templates" })).toBeVisible();
      await page.getByRole("button", { name: "Add a repository" }).click();
      const dlg = page.getByRole("dialog", { name: "Add a template repository" });
      await dlg.getByLabel("Repository address", { exact: true }).fill("https://github.com/acme/agent-templates/tree/main/templates");
      await dlg.getByRole("button", { name: "Check", exact: true }).click();
      const found = dlg.getByTestId("template-source-found");
      await expect(found).toContainText("github.com/acme/agent-templates");
      await expect(found).toContainText("main");
      await expect(found).toContainText("1 template");
      await expect(found).toContainText("Schedule manager");
      await dlg.getByRole("button", { name: "Add repository" }).click();
      await expect(dlg).toHaveCount(0);
      const table = page.getByRole("table", { name: "Template repositories of this instance" });
      await expect(table.getByText("github.com/acme/agent-templates")).toHaveCount(2);
      // the check stored nothing; the add sent what the check found
      expect(mock.writes.map((w) => w.key)).toEqual(["POST /v1/admin/agent-template-sources/preview", "POST /v1/admin/agent-template-sources"]);
      expect(mock.writes[0].body).toEqual({ url: "https://github.com/acme/agent-templates/tree/main/templates" });
      expect(mock.writes[1].body).toEqual({ url: "https://github.com/acme/agent-templates.git", ref: "main", path: "templates" });
    });

    test("check now, edit the schedule, remove the repository", async ({ page }) => {
      const mock = await openTemplateScreen(page, "tab", "light");
      // Check now: one read, the row keeps its words
      await page.getByRole("button", { name: "Check now Team templates" }).click();
      await expect(page.getByRole("button", { name: "Check now Team templates" })).toBeEnabled();
      await expect.poll(() => mock.writes.map((w) => w.key)).toEqual(["POST /v1/admin/agent-template-sources/00000000-0000-4000-8000-000000000800/check"]);
      // Edit: the address is fixed, the schedule changes
      await page.getByRole("button", { name: "Edit Team templates" }).click();
      const dlg = page.getByRole("dialog", { name: "Edit Team templates" });
      await expect(dlg.getByTestId("template-source-address")).toHaveText("https://github.com/acme/agent-templates.git");
      await dlg.getByLabel("Check for updates").selectOption({ label: "Every week" });
      await dlg.getByRole("button", { name: "Save" }).click();
      await expect(dlg).toHaveCount(0);
      expect(mock.writes.at(-1)).toEqual({ key: "PATCH /v1/admin/agent-template-sources/00000000-0000-4000-8000-000000000800", body: { update_interval: "1w" } });
      // Remove: the confirm says what stays
      await page.getByRole("button", { name: "Remove Team templates" }).click();
      const confirm = page.getByRole("dialog", { name: "Remove Team templates?" });
      await expect(confirm.getByText("Agents made from these templates keep working and do not change.")).toBeVisible();
      await confirm.getByRole("button", { name: "Remove repository" }).click();
      await expect(page.getByRole("table", { name: "Template repositories of this instance" }).getByText("github.com/acme/agent-templates")).toHaveCount(0);
      expect(mock.writes.at(-1)?.key).toBe("DELETE /v1/admin/agent-template-sources/00000000-0000-4000-8000-000000000800");
    });
  });
}
