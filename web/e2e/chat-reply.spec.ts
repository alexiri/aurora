import AxeBuilder from "@axe-core/playwright";
import { expect, test, type Page } from "@playwright/test";

import { DEFAULT, MINE, openChat, type Ev, type Fixture } from "./support/chat-mock";
import { overflow } from "./support/open";

// P5-CHAT-2 (D-228, C-725; docs/14 §Agent chat): reply to a message, the quote strip and its jump, permalinks,
// and Jump to latest. Opened at 1280 px and 390 px in both themes; axe with no rule disabled.

const at = (m: number) => new Date(Date.now() - m * 60_000).toISOString();
const many: Ev[] = [
  { seq: 1, type: "turn.queued", ts: at(300), payload: { turn_id: "t1", speaker_label: "Ada Lovelace", door: "dashboard", text: "First question about the release.", attachments: [] } },
  { seq: 2, type: "text", ts: at(299), payload: { turn_id: "t1", message_id: "m2", text: "The release is **ready** to ship." } },
  ...Array.from({ length: 24 }, (_, i): Ev => ({
    seq: 3 + i,
    type: i % 2 ? "text" : "turn.queued",
    ts: at(290 - i),
    payload: i % 2 ? { turn_id: `t${i}`, message_id: `m${i}`, text: `Filler answer number ${i} `.repeat(6) } : { turn_id: `t${i}`, speaker_label: "Ada Lovelace", door: "dashboard", text: `Filler question ${i}`, attachments: [] },
  })),
  { seq: 27, type: "turn.queued", ts: at(5), payload: { turn_id: "t27", speaker_label: "Ada Lovelace", door: "dashboard", text: "Thanks, one more thing.", attachments: [], reply_to: { seq: 2, author_label: "Daneel", author_kind: "agent", at: at(299), excerpt: "The release is **ready** to ship." } } },
  { seq: 28, type: "turn.queued", ts: at(4), payload: { turn_id: "t28", speaker_label: "Ada Lovelace", door: "dashboard", text: "And this one.", attachments: [], reply_to: { seq: 1, redacted: true } } },
];
const fx: Fixture = { ...DEFAULT, events: { [MINE.id]: many } };

async function serious(page: Page) {
  const { violations } = await new AxeBuilder({ page }).analyze();
  return violations.filter((v) => v.impact === "serious" || v.impact === "critical").map((v) => `${v.id}: ${v.nodes.map((n) => n.target.join(" ")).slice(0, 3).join(" | ")}`);
}

for (const scheme of ["light", "dark"] as const) {
  test.describe(`${scheme}, 1280 px`, () => {
    test.use({ viewport: { width: 1280, height: 900 } });

    test("reply to an agent message: bar, strip on the sent message, click jumps and highlights", async ({ page }) => {
      await openChat(page, scheme, fx);
      const log = page.getByRole("log");
      const msg = log.locator('[data-seq="2"]');
      await msg.scrollIntoViewIfNeeded();
      await msg.hover();
      await msg.getByRole("button", { name: "Reply" }).click();
      const bar = page.getByRole("group", { name: "Replying to" });
      await expect(bar).toContainText("Replying to Daneel");
      await expect(page.locator("#chat-input")).toBeFocused();
      await page.keyboard.press("Escape");
      await expect(bar).toHaveCount(0);

      // A sent message that quotes: the strip shows the author and excerpt; a redacted original is a neutral line.
      const strip = log.locator('[data-seq="27"]').getByTestId("quote-strip");
      await expect(strip).toContainText("Daneel");
      await expect(log.locator('[data-seq="28"]').getByTestId("quote-strip")).toHaveText("Original message redacted");
      await strip.click();
      await expect(log.locator('[data-seq="2"]')).toBeInViewport();
      await expect(log.locator('[data-seq="2"]')).toHaveClass(/bg-accent-soft/);
      await expect(log.locator('[data-seq="2"]')).not.toHaveClass(/bg-accent-soft/, { timeout: 5_000 });
      expect(await serious(page)).toEqual([]);
    });

    test("the sent reply carries reply_to.seq", async ({ page }) => {
      await openChat(page, scheme, fx);
      const log = page.getByRole("log");
      const msg = log.locator('[data-seq="2"]');
      await msg.scrollIntoViewIfNeeded();
      await msg.hover();
      await msg.getByRole("button", { name: "Reply" }).click();
      const sent = page.waitForRequest((r) => r.method() === "POST" && /\/messages$/.test(r.url()));
      await page.locator("#chat-input").fill("Ship it");
      await page.keyboard.press("Enter");
      const body = (await sent).postDataJSON() as { text: string; reply_to?: { seq: number } };
      expect(body).toMatchObject({ text: "Ship it", reply_to: { seq: 2 } });
    });

    test("a permalink opens the right message in a fresh page", async ({ page }) => {
      // The highlight lasts 2 s and a loaded machine can outlast it before an assertion runs: record that it was
      // applied (a MutationObserver from the first paint) instead of racing the timer.
      await page.addInitScript(() => {
        const w = window as unknown as { __highlighted?: boolean };
        new MutationObserver(() => {
          if (document.querySelector('[data-seq="2"].bg-accent-soft')) w.__highlighted = true;
        }).observe(document, { subtree: true, attributes: true, childList: true });
      });
      await openChat(page, scheme, fx, `?c=${MINE.id}&m=2`);
      const target = page.getByRole("log").locator('[data-seq="2"]');
      await expect(target).toBeInViewport();
      await page.waitForFunction(() => (window as unknown as { __highlighted?: boolean }).__highlighted === true);
      await expect.poll(() => new URL(page.url()).searchParams.has("m")).toBe(false);
    });

    test("an unknown message in a permalink is a neutral notice", async ({ page }) => {
      await openChat(page, scheme, fx, `?c=${MINE.id}&m=999`);
      await expect(page.getByText("Message not available.")).toBeVisible();
    });

    test("scrolled up, the pill appears, counts new messages, and returns to the bottom", async ({ page }) => {
      await openChat(page, scheme, fx);
      const log = page.getByRole("log");
      await log.evaluate((el) => (el.scrollTop = 0));
      const pill = page.getByRole("button", { name: /Jump to latest/ });
      await expect(pill).toBeVisible();
      await page.evaluate(() =>
        (window as unknown as { __es: { emit: (t: string, e: unknown) => void } }).__es.emit("text", {
          seq: 29, type: "text", ts: new Date().toISOString(), payload: { turn_id: "t28", message_id: "m29", text: "A new answer." },
        }),
      );
      await expect(pill).toContainText("1 new");
      await pill.click();
      await expect(pill).toHaveCount(0);
      await expect(log.getByText("A new answer.")).toBeInViewport();
    });
  });
}

test.describe("390 px", () => {
  test.use({ viewport: { width: 390, height: 844 } });
  test("reply bar and quote strip: no horizontal scroll, axe clean", async ({ page }) => {
    await openChat(page, "light", fx, `?c=${MINE.id}`);
    const log = page.getByRole("log");
    const msg = log.locator('[data-seq="2"]');
    await msg.scrollIntoViewIfNeeded();
    await msg.click();
    await msg.getByRole("button", { name: "Reply" }).click();
    await expect(page.getByRole("group", { name: "Replying to" })).toBeVisible();
    expect(await overflow(page)).toBeLessThanOrEqual(0);
    expect(await serious(page)).toEqual([]);
  });
});
