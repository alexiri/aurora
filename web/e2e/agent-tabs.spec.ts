import { expect, test } from "@playwright/test";

import { mockWithTerminal } from "./support/terminal";
import { CANVAS, overflow, type Scheme } from "./support/open";

// FIX-129 (docs/14 §Agent page, §Tabs): the agent page uses the available width, and its tab strip never
// scrolls sideways: tabs that do not fit collapse into a trailing "More" menu. Every tab, at 390, 768, 1280 and
// 1920 px, both themes: the page does not scroll horizontally and neither does the tablist. The API is mocked.

const TABS = ["chat", "instructions", "memory", "skills", "schedules", "webhooks", "peers", "channels", "previews", "terminal", "access", "computer"];

/** The shared mock plus the terminal socket, and the Access tab's grants (empty) the shared mock does not serve. */
async function mockAgent(page: import("@playwright/test").Page) {
  await mockWithTerminal(page, { output: new Uint8Array(), resizes: [] });
  await page.route("**/v1/agents/*/grants", (route) =>
    route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify({ secrets: [], connections: [] }) }),
  );
  await page.route("**/v1/agents/*/effective", (route) =>
    route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify({ credentials: [], tools: [] }) }),
  );
}

const noScroll = (page: import("@playwright/test").Page) =>
  page.getByRole("tablist", { name: "Agent" }).evaluate((el) => el.scrollWidth - el.clientWidth);

for (const scheme of ["light", "dark"] as const satisfies readonly Scheme[]) {
  for (const width of [390, 768, 1280, 1920]) {
    test.describe(`agent page tabs (${width} px, ${scheme})`, () => {
      test.use({ viewport: { width, height: 900 } });

      test("every tab: no horizontal scroll on the page or the tablist, selected tab visible", async ({ page }) => {
        const errors: string[] = [];
        page.on("pageerror", (e) => errors.push(e.message));
        await mockAgent(page);
        await page.emulateMedia({ colorScheme: scheme });
        for (const tab of TABS) {
          await page.goto(`/o/acme/w/main/agents/daneel?tab=${tab}`);
          await expect.poll(() => page.locator("html").getAttribute("data-theme"), { timeout: 20_000 }).toBe(scheme);
          const selected = page.getByRole("tablist", { name: "Agent" }).getByRole("tab", { selected: true });
          await expect(selected, `${tab}: a tab is selected`).toBeVisible();
          await page.waitForLoadState("networkidle");
          expect(await page.evaluate(() => getComputedStyle(document.body).backgroundColor)).toBe(CANVAS[scheme]);
          expect(await overflow(page), `${tab} @${width}: the page scrolls sideways`).toBeLessThanOrEqual(0);
          expect(await noScroll(page), `${tab} @${width}: the tablist scrolls sideways`).toBeLessThanOrEqual(0);
          // The selected tab is always one of the visible tabs (it never hides in the menu).
          await expect(page.getByRole("tablist", { name: "Agent" }).getByRole("tab", { selected: true })).toHaveCount(1);
        }
        expect(errors).toEqual([]);
      });
    });
  }
}

test.describe("agent page width", () => {
  test.use({ viewport: { width: 1280, height: 900 } });

  test("the content is wider than the old 896 px cap at 1280 px", async ({ page }) => {
    await mockAgent(page);
    await page.goto("/o/acme/w/main/agents/daneel?tab=skills");
    const panel = page.getByRole("tabpanel");
    await expect(panel).toBeVisible();
    await page.waitForLoadState("networkidle");
    const width = await panel.evaluate((el) => el.getBoundingClientRect().width);
    expect(width).toBeGreaterThan(896);
  });

  test("a narrow strip offers More, and choosing a hidden tab selects it and shows it", async ({ page }) => {
    await page.setViewportSize({ width: 768, height: 900 });
    await mockAgent(page);
    await page.goto("/o/acme/w/main/agents/daneel?tab=instructions");
    const more = page.getByRole("button", { name: /^More agent tabs/ });
    await expect(more).toBeVisible();
    await more.click();
    await page.getByRole("menuitem", { name: "Computer" }).click();
    await expect(page).toHaveURL(/tab=computer/);
    await expect(page.getByRole("tablist", { name: "Agent" }).getByRole("tab", { name: "Computer" })).toHaveAttribute("aria-selected", "true");
  });
});
