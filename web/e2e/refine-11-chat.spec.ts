import AxeBuilder from "@axe-core/playwright";
import { expect, test, type Page } from "@playwright/test";

import { DEFAULT, MINE, openChat } from "./support/chat-mock";
import { overflow } from "./support/open";

// P5-REFINE-11 (D-408, D-415; UX review 5 chat-1, chat-5, chat-6, chat-8, chat-11): the Settings button, the rail's visible
// words, an archived conversation (read-only, Restore, no composer), one composer row until Aa, the phone opening the
// conversation. Both themes at 1280 px and 390 px; axe (serious and critical) clean. The API is mocked.

async function serious(page: Page) {
  const { violations } = await new AxeBuilder({ page }).analyze();
  return violations.filter((v) => v.impact === "serious" || v.impact === "critical").map((v) => v.id);
}

for (const scheme of ["light", "dark"] as const) {
  test.describe(`${scheme}, 1280 px`, () => {
    test.use({ viewport: { width: 1280, height: 900 } });

    test("the header has Chat · Automations · Computer and a labelled Settings button", async ({ page }) => {
      await openChat(page, scheme, DEFAULT, `?c=${MINE.id}`);
      await expect(page.getByRole("tablist", { name: "Agent sections" }).getByRole("tab")).toHaveText(["Chat", "Automations", "Computer"]);
      await expect(page.getByRole("button", { name: "Settings" })).toBeVisible();
      expect(await serious(page)).toEqual([]);
    });

    test("the collapsed rail names its parts under the icons (Need · Now · Did)", async ({ page }) => {
      await page.addInitScript(() => window.localStorage.setItem("aurora.agent-status-rail", "collapsed"));
      await openChat(page, scheme, DEFAULT, `?c=${MINE.id}`);
      const rail = page.getByRole("complementary", { name: "Status" });
      await expect(rail.getByText("Now", { exact: true })).toBeVisible();
      await expect(rail.getByText("Did", { exact: true })).toBeVisible();
    });

    test("the composer is one row until Aa opens the formatting toolbar, and the choice is kept", async ({ page }) => {
      await openChat(page, scheme, DEFAULT, `?c=${MINE.id}`);
      const row = page.getByTestId("formatting-row");
      await expect(row).toBeHidden();
      await page.getByRole("button", { name: "Formatting" }).click();
      await expect(row).toBeVisible();
      await page.reload();
      await expect(page.getByTestId("formatting-row")).toBeVisible();
      await page.getByRole("button", { name: "Formatting" }).click();
      await expect(page.getByTestId("formatting-row")).toBeHidden();
    });

    test("an archived conversation is read-only: its header says so, Restore is there, the composer is gone", async ({ page }) => {
      await openChat(page, scheme, DEFAULT);
      await page.getByRole("tab", { name: "Archived" }).click();
      await page.getByRole("button", { name: /Notion handbook review/ }).click();
      await expect(page.getByText("read only")).toBeVisible();
      await expect(page.getByRole("button", { name: "Restore" })).toBeVisible();
      await expect(page.getByText("This conversation is archived. Restore it to write again.")).toBeVisible();
      await expect(page.locator("#chat-input")).toHaveCount(0);
      expect(await overflow(page)).toBeLessThanOrEqual(0);
      expect(await serious(page)).toEqual([]);
    });
  });

  test.describe(`${scheme}, 390 px`, () => {
    test.use({ viewport: { width: 390, height: 844 } });

    test("the phone opens the conversation with the composer ready; Conversations is one tap back", async ({ page }) => {
      await openChat(page, scheme, DEFAULT);
      await expect(page.getByRole("log")).toBeVisible();
      await expect(page.locator("#chat-input")).toBeVisible();
      await expect(page.getByRole("button", { name: "Settings" })).toBeVisible();
      expect(await overflow(page)).toBeLessThanOrEqual(0);
      await page.getByRole("button", { name: "Back to conversations" }).click();
      await expect(page.getByRole("navigation", { name: "Conversations" })).toBeVisible();
      expect(await overflow(page)).toBeLessThanOrEqual(0);
      expect(await serious(page)).toEqual([]);
    });
  });
}
