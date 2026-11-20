import { expect, test } from "@playwright/test";

import { serious } from "./support/axe";
import { openRoute, overflow } from "./support/open";

// P5-POLISH-6 (D-382, D-385): Behaviour and Access are configuration pages opened from the … menu, not tabs. No section tab is
// selected on them, the page has a back link, its name and one sentence; old `?tab=` links open them; Instructions is one
// card whose save bar appears only after a change. Both themes at 1280 px and 390 px, axe, no horizontal scroll, no console error.

const AGENT = "/o/acme/w/main/agents/daneel";

for (const scheme of ["light", "dark"] as const) {
  for (const width of [1280, 390]) {
    test.describe(`${scheme} theme, ${width} px`, () => {
      test.use({ viewport: { width, height: width === 1280 ? 900 : 844 } });

      test("the tab bar is Chat · Automations · Computer; Instructions and Apps & access open from the … menu", async ({ page }) => {
        await openRoute(page, AGENT, scheme);
        const tabs = page.getByRole("tablist", { name: "Agent sections" }).getByRole("tab");
        await expect(tabs).toHaveText(["Chat", "Automations", "Computer"]);
        await page.getByRole("button", { name: "Agent actions" }).click();
        await expect(page.getByRole("menuitem", { name: /Pause Daneel/ })).toBeVisible();
        await page.getByRole("menuitem", { name: /^Apps & access/ }).click();
        await expect(page.getByRole("heading", { name: "Apps & access", level: 2 })).toBeVisible();
        await expect(page.getByText("What Daneel can use: apps, accounts and its AI key.")).toBeVisible();
        await expect(page.getByRole("tablist", { name: "Agent sections" }).getByRole("tab", { selected: true })).toHaveCount(0);
        expect(await overflow(page)).toBeLessThanOrEqual(0);
        expect(await serious(page)).toEqual([]);
        await page.getByRole("button", { name: "Chat" }).click();
        await expect(page.getByRole("tab", { name: "Chat", selected: true })).toBeVisible();
      });

      test("old links ?tab=access and ?tab=instructions open the configuration pages", async ({ page }) => {
        await openRoute(page, `${AGENT}?tab=instructions`, scheme);
        await expect(page.getByRole("heading", { name: "Instructions", level: 2 })).toBeVisible();
        await expect(page.locator("#a-brief")).toBeVisible();
        expect(await overflow(page)).toBeLessThanOrEqual(0);
        expect(await serious(page)).toEqual([]);
        await openRoute(page, `${AGENT}?tab=access`, scheme);
        await expect(page.getByRole("heading", { name: "Apps & access", level: 2 })).toBeVisible();
      });

      test("Instructions: the save bar appears only after a change, and Discard puts it back", async ({ page }) => {
        await openRoute(page, `${AGENT}?tab=instructions`, scheme);
        const instructions = page.locator("#a-brief");
        await expect(page.getByRole("region", { name: "Unsaved changes" })).toHaveCount(0);
        await instructions.fill("Something new");
        const bar = page.getByRole("region", { name: "Unsaved changes" });
        await expect(bar).toBeVisible();
        await expect(bar.getByRole("button", { name: "Save" })).toBeVisible();
        await bar.getByRole("button", { name: "Discard" }).click();
        await expect(bar).toHaveCount(0);
      });
    });
  }
}
