import { expect, test } from "@playwright/test";

import { DEFAULT, MINE, openChat } from "./support/chat-mock";
import { overflow } from "./support/open";
import { serious } from "./support/axe";

// (retention-tooltip) The org's retention window never disappears on a desktop width: below 1280 px it stays inline
// in the thread header's meta line (the FIX-235 segment), and from 1280 px it is the who-can-read (visibility)
// line's tooltip — hover, keyboard focus and a touch tap open it; Escape, blur and a tap outside close it; while it
// shows, the note is the line's accessible description. The header keeps its pinned 64 px (agent-page-p5) and axe
// stays clean with the tooltip open and closed, in both themes. Mocked API throughout (`--project mocked` only).

const LINE = "All 2 people in this workspace can use this agent. Only you can read this chat.";
const NOTE = "Deleted after 90 days without activity";

const openThread = (page: import("@playwright/test").Page, scheme: "light" | "dark") =>
  openChat(page, scheme, { ...DEFAULT, events: { [MINE.id]: [] } }, `?c=${MINE.id}`);

for (const scheme of ["light", "dark"] as const) {
  test(`from 1280 px the retention note is the visibility line's tooltip, the header stays 64 px (${scheme})`, async ({ page }) => {
    await openThread(page, scheme);
    const line = page.getByRole("button", { name: LINE });
    await expect(line).toBeVisible();
    const tip = page.getByTestId("retention-tip");
    // Closed until the line is hovered, focused or tapped.
    await expect(tip).toHaveCount(0);

    // Hover opens it; Escape closes it again.
    await line.hover();
    await expect(tip).toContainText(NOTE);
    await page.keyboard.press("Escape");
    await expect(tip).toHaveCount(0);

    // Keyboard focus opens it; while it shows, the note is the line's accessible description; axe is clean.
    // Blur first: Escape may have left the focus on the line already, and refocusing it would be a no-op.
    await page.evaluate(() => (document.activeElement as HTMLElement | null)?.blur());
    await line.focus();
    await expect(tip).toContainText(NOTE);
    expect(await line.getAttribute("aria-describedby"), "the line describes itself with the note").toBe(await tip.getAttribute("id"));
    expect(await serious(page)).toEqual([]);
    // Tab moves the focus away: the blur closes the tooltip, and axe is clean closed too.
    await page.keyboard.press("Tab");
    await expect(tip).toHaveCount(0);
    expect(await serious(page)).toEqual([]);

    // The pinned 64 px header (agent-page-p5) is untouched by the tooltip's trigger.
    const header = page.getByRole("region", { name: "Chat" }).locator("header").first();
    expect(Math.round((await header.boundingBox())!.height), "the header is the 64 px one").toBe(64);
    expect(await overflow(page), "no sideways scroll").toBe(0);
  });
}

test.describe("touch (retention-tooltip)", () => {
  test.use({ viewport: { width: 1280, height: 900 }, hasTouch: true });

  test("a tap opens the tooltip, a tap outside closes it", async ({ page }) => {
    await openThread(page, "light");
    const line = page.getByRole("button", { name: LINE });
    const tip = page.getByTestId("retention-tip");
    await line.tap();
    await expect(tip).toContainText(NOTE);
    await page.getByRole("region", { name: "Chat" }).click({ position: { x: 10, y: 400 } });
    await expect(tip).toHaveCount(0);
  });
});

test.describe("390 px (retention-tooltip)", () => {
  test.use({ viewport: { width: 390, height: 844 } });

  test("the retention note stays inline in the meta line, and there is no tooltip and no sideways scroll", async ({ page }) => {
    await openThread(page, "light");
    const meta = page.getByTestId("thread-meta");
    await expect(meta).toContainText(NOTE);
    // Below 1280 px the visibility line is not a trigger: no button, no tooltip.
    expect(await page.getByRole("button", { name: LINE }).count()).toBe(0);
    await expect(page.getByTestId("retention-tip")).toHaveCount(0);
    expect(await overflow(page), "no sideways scroll").toBe(0);
  });
});
