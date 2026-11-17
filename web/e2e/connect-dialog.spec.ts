import { expect, test } from "@playwright/test";

import { openRoute, overflow } from "./support/open";

// FIX-149 (D-272, C-842; docs/14 §Key flows "Connect dialog", §Dialogs): Connect is a dialog on the page, like Details
// (not a dashboard page in a small browser window), and every dialog has a close icon. Both themes; at 390 px the dialog
// has no horizontal scroll and the close icon stays reachable while the dialog body scrolls.

const APPS = "/o/acme/w/main/apps?tab=apps";

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
        // The connect journey (P5-UX-3): its name follows the steps, so hold on to the dialog itself. The workspace has one
        // agent, so the Agent step comes first with Daneel chosen.
        const dialog = page.getByTestId("ws-connect-journey");
        await expect(dialog).toBeVisible();
        await expect(dialog.getByRole("group", { name: "Which agents can use GitHub?" })).toBeVisible();
        // The dialog sits on the page itself: the dashboard is still behind it, in this window.
        expect(page.url()).toContain("/apps?tab=apps");
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

      // (P5-UX-9, C-933) Details is the app's setup page now, not a dialog (e2e/app-setup.spec.ts).
      test("Details leads to the app's setup page and opens no dialog", async ({ page }) => {
        await openRoute(page, APPS, scheme);
        await page.getByRole("link", { name: "Details of GitHub" }).click();
        await expect(page).toHaveURL(/\/o\/acme\/w\/main\/apps\/github$/);
        await expect(page.getByRole("dialog")).toHaveCount(0);
      });

      test("the deep link opens the dialog and closing it removes the parameter", async ({ page }) => {
        await openRoute(page, `${APPS}&connect=github`, scheme);
        const dialog = page.getByTestId("ws-connect-journey");
        await expect(dialog).toBeVisible();
        await dialog.getByRole("button", { name: "Close" }).click();
        await expect(dialog).toBeHidden();
        await expect.poll(() => new URL(page.url()).searchParams.has("connect")).toBe(false);
      });

      test("the old connect route redirects to the dialog", async ({ page }) => {
        await openRoute(page, "/o/acme/w/main/apps/github/connect", scheme);
        await expect(page.getByTestId("ws-connect-journey")).toBeVisible();
        expect(new URL(page.url()).pathname).toBe("/o/acme/w/main/apps");
      });
    });
  }
}

test.describe("× stays reachable at 390 px", () => {
  test.use({ viewport: { width: 390, height: 600 } });
  test("scrolling a long dialog keeps the close icon in view", async ({ page }) => {
    await openRoute(page, `${APPS}&connect=github`, "light");
    const dialog = page.getByTestId("ws-connect-journey");
    await expect(dialog).toBeVisible();
    // Go to the sign-in step and open Advanced so the form is long enough to scroll.
    await dialog.getByRole("button", { name: "Give Daneel access" }).click();
    await dialog.getByRole("button", { name: "Next" }).click();
    await dialog.getByText(/^Advanced: other ways to connect/).click();
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
