import AxeBuilder from "@axe-core/playwright";
import { expect as baseExpect, test, type Page } from "@playwright/test";

import { overflow, openRoute } from "./support/open";

// The first request of a route compiles it in `next dev`; on a loaded host that outlasts the default 5 s.
const expect = baseExpect.configure({ timeout: 30_000 });

// P5-USAGE-2 (C-1004, C-1005; 14 §Usage): the usage view in its four places, mocked API (support/mock-api.ts).

async function axeClean(page: Page) {
  const { violations } = await new AxeBuilder({ page }).analyze();
  return violations
    .filter((v) => v.impact === "serious" || v.impact === "critical")
    .map((v) => `${v.id} (${v.impact}): ${v.nodes.map((n) => n.target.join(" ")).slice(0, 5).join(" | ")}`);
}

async function checked(page: Page) {
  expect(await overflow(page)).toBeLessThanOrEqual(0);
  expect(await axeClean(page)).toEqual([]);
}

for (const scheme of ["light", "dark"] as const) {
  for (const width of [1280, 390]) {
    test.describe(`${scheme} theme, ${width} px`, () => {
      test.use({ viewport: { width, height: 900 } });

      test("Activity › Usage: tiles, per-day chart, tables, table view", async ({ page }) => {
        await openRoute(page, "/o/acme/w/main/activity?tab=usage", scheme);
        await expect(page.getByRole("tab", { name: "Usage" })).toHaveAttribute("aria-selected", "true");
        await expect(page.getByText("Days are in UTC.")).toBeVisible();
        for (const label of ["Tokens", "Turns", "Turn time", "Awake time"]) await expect(page.getByText(label, { exact: true }).first()).toBeVisible();
        await expect(page.getByRole("heading", { name: "Tokens per day" })).toBeVisible();
        await expect(page.getByTestId("bar-chart")).toBeVisible();
        await expect(page.getByText("Deleted agent")).toBeVisible();
        await expect(page.getByText("Unknown model")).toBeVisible();
        await checked(page);
        // keyboard: focus the chart, move between bars
        await page.getByTestId("bar-chart").focus();
        await expect(page.getByTestId("bar-tooltip")).toBeVisible();
        await page.keyboard.press("ArrowLeft");
        await expect(page.getByTestId("bar-tooltip")).toBeVisible();
        await page.getByRole("button", { name: "Show as table" }).click();
        await expect(page.getByRole("table", { name: "Tokens per day" })).toBeVisible();
        await checked(page);
        // the range switch changes the URL
        await page.getByLabel("7 days").check({ force: true });
        await expect(page).toHaveURL(/range=7d/);
      });

      test("Activity opens on Requests; an agent's See usage filters Usage to it", async ({ page }) => {
        await openRoute(page, "/o/acme/w/main/activity", scheme);
        await expect(page.getByRole("tab", { name: "Requests" })).toHaveAttribute("aria-selected", "true");
        await openRoute(page, "/o/acme/w/main/activity?tab=usage&agent=00000000-0000-4000-8000-000000000011", scheme);
        await expect(page.getByRole("heading", { name: "Usage of Daneel" })).toBeVisible();
        await expect(page.getByRole("heading", { name: "By agent" })).toHaveCount(0);
        await checked(page);
      });

      test("the agent's Computer page shows the Usage card with See usage", async ({ page }) => {
        // (P5-REFINE-20 #11) The Usage card is on the Computer's Details (`?tab=computer` opens on Files).
        await openRoute(page, "/o/acme/w/main/agents/daneel?tab=details", scheme);
        const card = page.getByRole("region", { name: "Usage" });
        await expect(card).toBeVisible();
        await expect(card.getByRole("link", { name: "See usage" })).toHaveAttribute("href", /\/activity\?tab=usage&agent=/);
        await checked(page);
      });

      test("the organization's Data › Usage and Administration › Data › Usage", async ({ page }) => {
        await openRoute(page, "/o/acme/data?tab=usage", scheme);
        await expect(page.getByRole("tab")).toHaveText(["Retention", "Storage", "Audit log", "Usage"]);
        await expect(page.getByTestId("usage-view").getByLabel("Workspace", { exact: true })).toBeVisible();
        await expect(page.getByRole("heading", { name: "Top workspaces" })).toBeVisible();
        await checked(page);
        await openRoute(page, "/admin/data?tab=usage", scheme);
        await expect(page.getByTestId("usage-view").getByLabel("Organization", { exact: true })).toBeVisible();
        await expect(page.getByRole("heading", { name: "By organization" })).toBeVisible();
        await expect(page.getByText("Other workspaces")).toBeVisible();
        await checked(page);
      });
    });
  }
}
