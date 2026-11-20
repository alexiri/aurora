import { expect, test, type Locator, type Page } from "@playwright/test";

import { openJourney } from "./support/journey";
import { openRoute } from "./support/open";

// P5-POLISH-1 (D-393, UX review 4 A1): a dialog is three rows: a header that never scrolls, a body that does, an optional
// footer. The body's first element starts at or below the header's bottom (the old sticky title row drew 24 px over it
// and cut the connect journey's step labels in half), the close button stays inside the viewport at the end of the
// scroll, and opening and closing dialogs and menus logs no console error. Both themes at 1280 px and 390 px.

/** Fails the test on a console error, a page error or a CSP violation (the production spec covers the real CSP). */
function watchConsole(page: Page) {
  const bad: string[] = [];
  page.on("console", (m) => {
    // "Failed to load resource" is a request the mock leaves unanswered, not a page error.
    if (m.type() === "error" && !/^Failed to load resource/.test(m.text())) bad.push(m.text());
  });
  page.on("pageerror", (e) => bad.push(e.message));
  return bad;
}

async function checkDialog(page: Page, dialog: Locator) {
  await expect(dialog).toBeVisible();
  const header = dialog.locator("> div").first();
  const body = dialog.locator("> div").nth(1);
  // Measure from the top of the body: filling a field scrolls it into view (the sign-in step is long since P5-POLISH-5),
  // and what scrolled up is clipped by the body, not drawn over the header.
  await body.evaluate((el) => el.scrollTo(0, 0));
  const headerBox = (await header.boundingBox())!;
  const firstVisible = body.locator(":scope > *").first();
  const first = (await firstVisible.boundingBox())!;
  expect(first.y, "the body's first element starts below the header").toBeGreaterThanOrEqual(headerBox.y + headerBox.height - 0.5);
  const bodyBox = (await body.boundingBox())!;
  expect(bodyBox.y, "the body starts below the header").toBeGreaterThanOrEqual(headerBox.y + headerBox.height - 0.5);
  await body.evaluate((el) => el.scrollTo(0, el.scrollHeight));
  // The header's × (the Done step's footer has a "Close" button too since P5-POLISH-5).
  const close = dialog.getByRole("button", { name: "Close" }).first();
  await expect(close).toBeVisible();
  const box = (await close.boundingBox())!;
  const vp = page.viewportSize()!;
  expect(box.y).toBeGreaterThanOrEqual(0);
  expect(box.y + box.height).toBeLessThanOrEqual(vp.height);
  expect(box.x + box.width).toBeLessThanOrEqual(vp.width);
  await body.evaluate((el) => el.scrollTo(0, 0));
}

for (const scheme of ["light", "dark"] as const) {
  for (const width of [1280, 390]) {
    test.describe(`${scheme} theme, ${width} px`, () => {
      test.use({ viewport: { width, height: width === 1280 ? 900 : 700 } });

      test("the connect journey keeps its step labels clear of the header, on every step", async ({ page }) => {
        const bad = watchConsole(page);
        await openJourney(page, "/o/acme/w/main/agents/daneel?tab=access", scheme);
        await page.getByRole("button", { name: "Give Daneel an app" }).first().click();
        const dialog = page.getByTestId("ws-connect-journey");
        await checkDialog(page, dialog);
        // (P5-POLISH-5, D-388) The journey's title is "<App> for <Agent>" (connect-journey.spec does the same).
        await dialog.getByRole("button", { name: "GitHub", exact: true }).click();
        await expect(page.getByRole("dialog", { name: "GitHub for Daneel" })).toBeVisible();
        const steps = dialog.getByRole("list", { name: "Steps" });
        await expect(steps).toBeVisible();
        await checkDialog(page, dialog);
        await dialog.getByRole("button", { name: "Next" }).click();
        await dialog.getByLabel("Personal access token", { exact: true }).fill("not-a-real-token");
        await checkDialog(page, dialog);
        await dialog.getByRole("button", { name: "Connect", exact: true }).click();
        await expect(dialog.getByText("Daneel can now read acme.")).toBeVisible();
        await checkDialog(page, dialog);
        // The Done step's "Try it in chat" link has readable text: its colour is not its background.
        const open = dialog.getByRole("link", { name: "Try it in chat" });
        await expect(open).toBeVisible();
        const [fg, bg] = await open.evaluate((el) => {
          const s = getComputedStyle(el);
          return [s.color, s.backgroundColor];
        });
        expect(fg).not.toBe(bg);
        expect(bad).toEqual([]);
      });

      test("the apps and accounts journey and the org journey open clear of the header", async ({ page }) => {
        const bad = watchConsole(page);
        await openRoute(page, "/o/acme/w/main/apps?tab=accounts&connect=", scheme);
        await checkDialog(page, page.getByTestId("ws-connect-journey"));
        expect(bad).toEqual([]);
      });

      test("a confirm dialog (Delete this agent, from Settings) opens and closes without a console error or ring", async ({ page }) => {
        const bad = watchConsole(page);
        await openRoute(page, "/o/acme/w/main/agents/daneel", scheme);
        await page.getByRole("button", { name: "Settings" }).click();
        await page.getByRole("button", { name: "Delete Daneel…" }).click();
        const dialog = page.getByRole("dialog", { name: "Delete this agent" });
        await checkDialog(page, dialog);
        await dialog.getByRole("button", { name: "Close" }).click();
        await expect(dialog).toBeHidden();
        expect(bad).toEqual([]);
      });
    });
  }
}
