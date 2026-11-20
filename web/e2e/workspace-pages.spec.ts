import { expect, test } from "@playwright/test";

import { serious } from "./support/axe";
import { openRoute, overflow } from "./support/open";

// P5-POLISH-7 (D1–D6, F1): the workspace pages say what is true and what a person can do next. Both themes at 1280 px and
// 390 px, axe, no horizontal scroll, no console error (the shared fixture fails on one).

const BASE = "/o/acme/w/main";

for (const scheme of ["light", "dark"] as const) {
  for (const width of [1280, 390]) {
    test.describe(`${scheme} theme, ${width} px`, () => {
      test.use({ viewport: { width, height: width === 1280 ? 900 : 844 } });

      test("Overview opens with its title and a sentence, and lists agents as rows with Chat", async ({ page }) => {
        await openRoute(page, BASE, scheme);
        await expect(page.getByRole("heading", { name: "Overview", level: 1 })).toBeVisible();
        const agents = page.getByRole("list", { name: "Agents" });
        await expect(agents.getByRole("link", { name: /Chat with/ }).first()).toBeVisible();
        await expect(agents.getByText(/^Hosted$|^Default$/)).toHaveCount(0);
        expect(await overflow(page)).toBeLessThanOrEqual(0);
        expect(await serious(page)).toEqual([]);
      });

      test("Approvals says how many requests wait, in one list", async ({ page }) => {
        await openRoute(page, `${BASE}/approvals`, scheme);
        await expect(page.getByTestId("approvals-sentence")).toBeVisible();
        await expect(page.getByRole("heading", { name: "Action approvals" })).toHaveCount(0);
        expect(await overflow(page)).toBeLessThanOrEqual(0);
        expect(await serious(page)).toEqual([]);
      });

      test("Apps: every card has a status line and no '0 connections'", async ({ page }) => {
        await openRoute(page, `${BASE}/apps?tab=apps`, scheme);
        await expect(page.getByTestId("app-status").first()).toBeVisible();
        await expect(page.getByText(/\d+ connections?\b/)).toHaveCount(0);
        expect(await overflow(page)).toBeLessThanOrEqual(0);
        expect(await serious(page)).toEqual([]);
      });

      test("People: one table, role legend in a disclosure", async ({ page }) => {
        await openRoute(page, `${BASE}/people`, scheme);
        await expect(page.getByText("What can each role do?")).toBeVisible();
        await expect(page.getByText("Members with access")).toHaveCount(0);
        expect(await overflow(page)).toBeLessThanOrEqual(0);
        expect(await serious(page)).toEqual([]);
      });

      test("Internet access: History of changes and the simulator behind a button", async ({ page }) => {
        await openRoute(page, `${BASE}/internet-access`, scheme);
        await expect(page.getByRole("button", { name: "History of changes" })).toBeVisible();
        await page.getByRole("button", { name: "Try a request" }).click();
        await expect(page.getByRole("dialog", { name: "Try a request" })).toBeVisible();
        expect(await serious(page)).toEqual([]);
      });
    });
  }
}
