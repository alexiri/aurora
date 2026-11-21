import { expect, test } from "@playwright/test";

import { serious } from "./support/axe";
import { openRoute, overflow } from "./support/open";

// P5-REFINE-11 (D-408, was P5-POLISH-6 D-382): the Settings pages open from the labelled Settings button, not tabs. No section tab is
// selected on them, the page has a back link, its name and one sentence; old `?tab=` links open them; Instructions is one
// card whose save bar appears only after a change. Both themes at 1280 px and 390 px, axe, no horizontal scroll, no console error.

const AGENT = "/o/acme/w/main/agents/daneel";

for (const scheme of ["light", "dark"] as const) {
  for (const width of [1280, 390]) {
    test.describe(`${scheme} theme, ${width} px`, () => {
      test.use({ viewport: { width, height: width === 1280 ? 900 : 844 } });

      test("the tab bar is Chat · Automations · Computer; the labelled Settings button opens Settings; Pause and Delete are on its last page, Manage, and under no other page", async ({ page }) => {
        await openRoute(page, AGENT, scheme);
        const tabs = page.getByRole("tablist", { name: "Agent sections" }).getByRole("tab");
        await expect(tabs).toHaveText(["Chat", "Automations", "Computer"]);
        await page.getByRole("button", { name: "Settings" }).click();
        await expect(page.getByRole("heading", { name: "Instructions", level: 2 })).toBeVisible();
        await expect(page.getByRole("button", { name: "Pause Daneel" })).toHaveCount(0);
        await expect(page.getByRole("button", { name: "Delete Daneel…" })).toHaveCount(0);
        await page.getByRole("tablist", { name: "Settings pages" }).getByRole("tab", { name: "Apps & access" }).click();
        await expect(page.getByRole("heading", { name: "Apps & access", level: 2 })).toBeVisible();
        await expect(page.getByText("Apps, accounts and the AI key it uses")).toBeVisible();
        await expect(page.getByRole("button", { name: "Pause Daneel" })).toHaveCount(0);
        await page.getByRole("tablist", { name: "Settings pages" }).getByRole("tab", { name: "Manage" }).click();
        await expect(page.getByRole("heading", { name: "Manage", level: 2 })).toBeVisible();
        await expect(page.getByRole("button", { name: "Pause Daneel" })).toBeVisible();
        await expect(page.getByRole("button", { name: "Delete Daneel…" })).toBeVisible();
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

      test("Instructions uses the width (FIX-212): from md picture and Name, Model and the message-first switch share a line; it stacks at 390 px", async ({ page }) => {
        await openRoute(page, `${AGENT}?tab=instructions`, scheme);
        const name = (await page.locator("#a-name").boundingBox())!;
        const picture = (await page.getByRole("button", { name: "Change picture" }).boundingBox())!;
        const model = (await page.locator("#a-model").boundingBox())!;
        const proactive = (await page.locator("#a-proactive").boundingBox())!;
        const brief = (await page.locator("#a-brief").boundingBox())!;
        if (width >= 768) {
          expect(Math.abs(name.y - picture.y)).toBeLessThan(80);
          expect(name.x).toBeGreaterThan(picture.x);
          expect(model.x + model.width).toBeLessThan(proactive.x);
          expect(Math.abs(model.y - proactive.y)).toBeLessThan(24);
          expect(model.width).toBeLessThanOrEqual(330);
          expect(brief.height).toBeGreaterThan(250);
        } else {
          expect(name.y).toBeGreaterThan(picture.y + picture.height);
          expect(proactive.y).toBeGreaterThan(model.y + model.height);
        }
        expect(await overflow(page)).toBeLessThanOrEqual(0);
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
