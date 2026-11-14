import { expect, test } from "@playwright/test";

import { openRoute, overflow } from "./support/open";

// FIX-149 (D-272, C-842; docs/14 §Key flows "Connect dialog", §Dialogs): Connect is a dialog on the page, like Details
// (not a dashboard page in a small browser window), and every dialog has a close icon. Both themes; at 390 px the dialog
// has no horizontal scroll and the close icon stays reachable while the dialog body scrolls.

const APPS = "/o/acme/w/main/credentials?tab=apps";

for (const scheme of ["light", "dark"] as const) {
  for (const width of [1280, 390]) {
    test.describe(`${scheme} theme, ${width} px`, () => {
      test.use({ viewport: { width, height: width === 1280 ? 900 : 844 } });

      test("Connect opens a dialog in the same page and closes with × and with Escape", async ({ page, context }) => {
        const opened: string[] = [];
        context.on("page", (p) => opened.push(p.url()));
        page.on("popup", (p) => opened.push(p.url()));
        await openRoute(page, APPS, scheme);
        await page.getByRole("button", { name: "Connect GitHub" }).click();
        const dialog = page.getByRole("dialog", { name: "Connect GitHub" });
        await expect(dialog).toBeVisible();
        await expect(dialog.getByRole("radiogroup", { name: "How do you want to connect?" })).toBeVisible();
        // The dialog sits on the page itself: the dashboard is still behind it, in this window.
        expect(page.url()).toContain("/credentials");
        expect(opened).toEqual([]);
        expect(await overflow(page), "horizontal page scroll").toBeLessThanOrEqual(0);
        expect(await dialog.evaluate((el) => el.scrollWidth - el.clientWidth), "horizontal dialog scroll").toBeLessThanOrEqual(0);

        await dialog.getByRole("button", { name: "Close" }).click();
        await expect(dialog).toBeHidden();

        await page.getByRole("button", { name: "Connect GitHub" }).click();
        await expect(dialog).toBeVisible();
        await page.keyboard.press("Escape");
        await expect(dialog).toBeHidden();
        expect(opened).toEqual([]);
      });

      test("Details shows the close icon too", async ({ page }) => {
        await openRoute(page, APPS, scheme);
        await page.getByRole("button", { name: "Details of GitHub" }).click();
        const dialog = page.getByRole("dialog", { name: "GitHub" });
        const close = dialog.getByRole("button", { name: "Close" });
        await expect(close).toBeVisible();
        await close.click();
        await expect(dialog).toBeHidden();
      });

      test("the deep link opens the dialog and closing it removes the parameter", async ({ page }) => {
        await openRoute(page, `${APPS}&connect=github`, scheme);
        const dialog = page.getByRole("dialog", { name: "Connect GitHub" });
        await expect(dialog).toBeVisible();
        await dialog.getByRole("button", { name: "Close" }).click();
        await expect(dialog).toBeHidden();
        await expect.poll(() => new URL(page.url()).searchParams.has("connect")).toBe(false);
      });

      test("the old connect route redirects to the dialog", async ({ page }) => {
        await openRoute(page, "/o/acme/w/main/apps/github/connect", scheme);
        await expect(page.getByRole("dialog", { name: "Connect GitHub" })).toBeVisible();
        expect(new URL(page.url()).pathname).toBe("/o/acme/w/main/credentials");
      });
    });
  }
}

test.describe("× stays reachable at 390 px", () => {
  test.use({ viewport: { width: 390, height: 600 } });
  test("scrolling a long dialog keeps the close icon in view", async ({ page }) => {
    await openRoute(page, `${APPS}&connect=github`, "light");
    const dialog = page.getByRole("dialog", { name: "Connect GitHub" });
    await expect(dialog).toBeVisible();
    // Choose the token method so the form is long enough to scroll.
    const scrollable = await dialog.evaluate((el) => el.scrollHeight - el.clientHeight);
    expect(scrollable, "the dialog body should scroll in this viewport").toBeGreaterThan(0);
    await dialog.evaluate((el) => { el.scrollTop = el.scrollHeight; });
    expect(await dialog.evaluate((el) => el.scrollTop)).toBeGreaterThan(0);
    const close = dialog.getByRole("button", { name: "Close" });
    await expect(close).toBeInViewport();
    const box = (await close.boundingBox())!;
    const frame = (await dialog.boundingBox())!;
    expect(box.y).toBeGreaterThanOrEqual(frame.y - 1);
    expect(box.y + box.height).toBeLessThanOrEqual(frame.y + frame.height + 1);
    await close.click();
    await expect(dialog).toBeHidden();
  });
});
