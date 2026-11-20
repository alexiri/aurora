import AxeBuilder from "@axe-core/playwright";
import { expect, test } from "@playwright/test";

import { EVAL_SCREENS, openEvals } from "./support/evals";
import { overflow } from "./support/open";

// P4-EVAL-4: the Evals page of the agent's Behaviour section, in both themes at 1280 px and 390 px: it renders, its
// tables (a stacked list on a phone) and a run's configuration difference fit with no horizontal page scroll, axe finds
// no serious or critical violation, and nothing the page shows is kept in storage (SEC-116). The same screens also run
// in stage4-axe, stage4-routes and stage2-narrow.

for (const scheme of ["light", "dark"] as const) {
  for (const width of [1280, 390]) {
    test.describe(`${scheme} theme, ${width} px`, () => {
      test.use({ viewport: { width, height: width === 1280 ? 900 : 844 } });

      test("the page lists questions, the judge, runs and schedules", async ({ page }) => {
        await openEvals(page, "page", scheme);
        await expect(page.getByRole("tab", { name: "Tests" }).first()).toBeVisible();
        for (const h of ["Questions", "Grader", "Runs", "Schedules"]) await expect(page.getByRole("heading", { name: h, level: 2 })).toBeVisible();
        await expect(page.getByText("3 of 100 questions, 3 included in runs.", { exact: false })).toBeVisible();
        await expect(page.getByLabel("Grader key")).toBeVisible();
        // The newest run dropped 12.5 points: a notice with counts only.
        await expect(page.getByTestId("regression-notice")).toContainText("fell by 12.5 points");
        // On a phone the tables are stacked lists with the same content; the table that is hidden is not announced twice.
        const questions = width === 1280 ? page.getByRole("region", { name: "Eval questions" }) : page.getByRole("list", { name: "Eval questions" });
        await expect(questions).toBeVisible();
        await expect(questions.getByText("What is the name of the team's release branch?")).toBeVisible();
        expect(await overflow(page)).toBeLessThanOrEqual(0);
        // Nothing of the page lands in storage or the URL.
        const stored = await page.evaluate(() => JSON.stringify([{ ...localStorage }, { ...sessionStorage }]));
        expect(stored).not.toMatch(/release\/2026|ANSWER/);
        expect(page.url()).not.toMatch(/release|answer/i);
        const { violations } = await new AxeBuilder({ page }).analyze();
        expect(violations.filter((v) => v.impact === "serious" || v.impact === "critical").map((v) => v.id)).toEqual([]);
      });

      test("a run's page shows what changed since the run it dropped from", async ({ page }) => {
        await openEvals(page, "run", scheme);
        await expect(page.locator("text=Model >> visible=true").first()).toBeVisible();
        await expect(page.locator("text=claude-sonnet-5-5 >> visible=true").first()).toBeVisible();
        await expect(page.locator("text=Instructions (hash) >> visible=true").first()).toBeVisible();
        // A reason in words, never a code.
        await expect(page.locator("text=Daneel's reply had no ANSWER line, so Aurora could not compare it. The reply is shown below. >> visible=true").first()).toBeVisible();
        expect(await overflow(page)).toBeLessThanOrEqual(0);
        const { violations } = await new AxeBuilder({ page }).analyze();
        expect(violations.filter((v) => v.impact === "serious" || v.impact === "critical").map((v) => v.id)).toEqual([]);
      });

      for (const screen of ["question-dialog", "schedule-dialog"] as const) {
        test(`${screen} fits and has no serious violations`, async ({ page }) => {
          await openEvals(page, screen, scheme);
          const dialog = page.getByRole("dialog");
          await expect(dialog).toBeVisible();
          if (screen === "schedule-dialog") await expect(dialog.getByTestId("cron-preview")).toContainText("Every day at 06:00");
          expect(await overflow(page)).toBeLessThanOrEqual(0);
          const { violations } = await new AxeBuilder({ page }).analyze();
          expect(violations.filter((v) => v.impact === "serious" || v.impact === "critical").map((v) => v.id)).toEqual([]);
        });
      }
    });
  }
}

test("the screens list is the one the other suites run", () => {
  expect([...EVAL_SCREENS]).toEqual(["page", "run", "question-dialog", "schedule-dialog"]);
});
