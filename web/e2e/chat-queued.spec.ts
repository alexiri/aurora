import { expect, test } from "@playwright/test";

import { DEFAULT, MINE, openChat, queued, type Fixture } from "./support/chat-mock";
import { overflow } from "./support/open";

// FIX-225 (D-469, D-470): the queued message above the composer. Steer, Edit → Save and Cancel against the mocked API, at 390 px
// and 1280 px in both themes: only the buttons the server allows, every click answered on the row, no horizontal scroll.

const fixture = (over: Partial<Fixture> = {}): Fixture => ({ ...DEFAULT, followups: [queued()], ...over });
const url = `?c=${MINE.id}`;
const row = (page: import("@playwright/test").Page) => page.locator("div.border-dashed", { hasText: "Queued" });

for (const scheme of ["light", "dark"] as const) {
  for (const [name, viewport] of [
    ["1280 px", { width: 1280, height: 900 }],
    ["390 px", { width: 390, height: 844 }],
  ] as const) {
    test.describe(`${scheme}, ${name}`, () => {
      test.use({ viewport });

      test("a queued message shows Steer, Edit and Cancel (never Send now); Steer says it is steering", async ({ page }) => {
        await openChat(page, scheme, fixture(), url);
        const r = row(page);
        await expect(r).toBeVisible();
        await expect(r.getByText("also check the Sentry alerts")).toBeVisible();
        await expect(r.getByText("Queued", { exact: true })).toBeVisible();
        await expect(r.getByRole("button")).toHaveText(["Steer", "Edit", "Cancel"]);
        await expect(page.getByRole("button", { name: "Send now" })).toHaveCount(0);
        if (viewport.width === 390) {
          // The buttons wrap under the text, and the page does not scroll sideways.
          const text = await r.getByText("also check the Sentry alerts").boundingBox();
          const steer = await r.getByRole("button", { name: "Steer" }).boundingBox();
          expect(steer!.y).toBeGreaterThanOrEqual(text!.y + text!.height - 1);
          const box = await r.boundingBox();
          expect(box!.x + box!.width).toBeLessThanOrEqual(viewport.width);
        }
        expect(await overflow(page)).toBeFalsy();
        const asked = page.waitForRequest((q) => q.method() === "POST" && /\/v1\/followups\/[^/]+\/steer$/.test(q.url()));
        await r.getByRole("button", { name: "Steer" }).click();
        await asked;
        await expect(r.getByText("Steering in at the next step…")).toBeVisible();
        await expect(r.getByRole("button")).toHaveText(["Edit", "Cancel"]);
        expect(await overflow(page)).toBeFalsy();
      });

      test("Edit opens a text box; Save sends the new text and the row shows it", async ({ page }) => {
        await openChat(page, scheme, fixture(), url);
        const r = row(page);
        await r.getByRole("button", { name: "Edit" }).click();
        const box = r.getByLabel("Edit queued message");
        await expect(box).toHaveValue("also check the Sentry alerts");
        await expect(r.getByRole("button")).toHaveText(["Save", "Discard changes"]);
        await box.fill("also check Sentry and Linear");
        expect(await overflow(page)).toBeFalsy();
        const saved = page.waitForRequest((q) => q.method() === "PATCH" && /\/v1\/followups\/[^/]+$/.test(q.url()));
        await r.getByRole("button", { name: "Save" }).click();
        expect(((await saved).postDataJSON() as { text: string }).text).toBe("also check Sentry and Linear");
        await expect(r.getByLabel("Edit queued message")).toHaveCount(0);
        await expect(r.getByText("also check Sentry and Linear")).toBeVisible();
      });

      test("Discard changes closes the box and keeps the text", async ({ page }) => {
        await openChat(page, scheme, fixture(), url);
        const r = row(page);
        await r.getByRole("button", { name: "Edit" }).click();
        await r.getByLabel("Edit queued message").fill("something else");
        await r.getByRole("button", { name: "Discard changes" }).click();
        await expect(r.getByText("also check the Sentry alerts")).toBeVisible();
      });

      test("Cancel removes the row", async ({ page }) => {
        await openChat(page, scheme, fixture(), url);
        const r = row(page);
        const cancelled = page.waitForRequest((q) => q.method() === "POST" && /\/v1\/followups\/[^/]+\/cancel$/.test(q.url()));
        await r.getByRole("button", { name: "Cancel" }).click();
        await cancelled;
        await expect(page.getByText("also check the Sentry alerts")).toHaveCount(0);
      });

      test("a refused action is written on the row, never silent", async ({ page }) => {
        await openChat(
          page,
          scheme,
          fixture({ followupErrors: { steer: { status: 409, code: "steer_unsupported", message: "nope" } } }),
          url,
        );
        const r = row(page);
        await r.getByRole("button", { name: "Steer" }).click();
        await expect(r.getByRole("alert")).toHaveText("This agent can't take messages while it works. The message runs when the current turn ends.");
        await expect(r.getByRole("button", { name: "Steer" })).toBeVisible();
        expect(await overflow(page)).toBeFalsy();
      });

      test("only the controls the server allows: Send now alone, or no Steer for someone else's message", async ({ page }) => {
        await openChat(
          page,
          scheme,
          fixture({ followups: [queued({ controls: { steer: false, send_now: true, edit: false, cancel: true } })] }),
          url,
        );
        await expect(row(page).getByRole("button")).toHaveText(["Send now", "Cancel"]);
      });
    });
  }
}
