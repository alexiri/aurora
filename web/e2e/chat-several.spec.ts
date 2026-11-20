import AxeBuilder from "@axe-core/playwright";
import { expect, test, type Page } from "@playwright/test";

import { conv, DEFAULT, MINE, openChat, type Ev, type Fixture } from "./support/chat-mock";
import { overflow } from "./support/open";

// P5-REFINE-11 after P5-REFINE-7 (D-409, C-1073..C-1075; UX review 5 chat-3, chat-4): several chats per person (New chat, the
// list, Rename), run threads folded under their automation, and the mark Stop leaves. Both themes at 1280 px and 390 px;
// axe (serious and critical) clean. The API is mocked.

async function serious(page: Page) {
  const { violations } = await new AxeBuilder({ page }).analyze();
  return violations.filter((v) => v.impact === "serious" || v.impact === "critical").map((v) => v.id);
}

const MORNING = { kind: "schedule", id: "11111111-1111-4111-8111-111111111111", name: "Morning summary", deleted: false };
const run = (n: number, daysAgo: number) =>
  conv(40 + n, "schedule", {
    title: "Morning summary",
    title_source: "automation",
    automation: MORNING,
    run_at: new Date(Date.now() - daysAgo * 86_400_000).toISOString(),
    last_activity_at: new Date(Date.now() - daysAgo * 86_400_000).toISOString(),
  });

const withFolder = (): Fixture => ({
  ...DEFAULT,
  active: [...DEFAULT.active.map((c) => ({ ...c })), conv(7, "direct", { direct_user_id: MINE.direct_user_id, title: "Lighthouse story", last_activity_at: new Date(Date.now() - 86_400_000).toISOString() })],
  folders: [{ kind: "schedule", id: MORNING.id, name: "Morning summary", deleted: false, thread_count: 3, last_activity_at: new Date().toISOString(), last_result: "answered" }],
  runs: [run(1, 1), run(2, 2), run(3, 3)],
});

const stopped = (): Fixture => {
  const events: Ev[] = [
    { seq: 1, type: "turn.queued", ts: new Date(Date.now() - 60_000).toISOString(), payload: { turn_id: "t1", speaker_label: "Ada Lovelace", door: "dashboard", text: "Write me a really long story about a lighthouse keeper." } },
    { seq: 2, type: "text", ts: new Date(Date.now() - 50_000).toISOString(), payload: { turn_id: "t1", message_id: "m2", text: "Chapter 1. Mara climbed the 112 steps to the lamp room before dawn…", partial: true } },
    { seq: 3, type: "turn.finished", ts: new Date(Date.now() - 40_000).toISOString(), payload: { turn_id: "t1", status: "aborted" } },
    { seq: 4, type: "notice", ts: new Date(Date.now() - 40_000).toISOString(), payload: { code: "turn_stopped", text: "You stopped this answer.", turn_id: "t1" } },
  ];
  return { ...DEFAULT, events: { [MINE.id]: events } };
};

for (const scheme of ["light", "dark"] as const) {
  test.describe(`${scheme}, 1280 px`, () => {
    test.use({ viewport: { width: 1280, height: 900 } });

    test("New chat opens a fresh conversation with the composer focused; the others stay in the list", async ({ page }) => {
      await openChat(page, scheme, withFolder(), `?c=${MINE.id}`);
      const list = page.getByRole("navigation", { name: "Conversations" });
      await expect(list.getByRole("button", { name: "New chat" })).toBeVisible();
      await list.getByRole("button", { name: "New chat" }).click();
      await expect(page.getByRole("heading", { name: "Say hello to Daneel" })).toBeVisible();
      await expect(page).toHaveURL(/\?c=/);
      await expect(page.locator("#chat-input")).toBeFocused();
      await expect(list.getByRole("button", { name: /^Lighthouse story/ })).toBeVisible();
      expect(await serious(page)).toEqual([]);
    });

    test("Rename edits the title in the list row: Enter saves", async ({ page }) => {
      await openChat(page, scheme, withFolder(), `?c=${MINE.id}`);
      const list = page.getByRole("navigation", { name: "Conversations" });
      await list.getByRole("button", { name: "Actions for Lighthouse story" }).click();
      await page.getByRole("menuitem", { name: "Rename" }).click();
      const field = list.getByLabel("Conversation title");
      await expect(field).toBeFocused();
      await field.fill("Lighthouse chapters");
      await field.press("Enter");
      await expect(list.getByRole("button", { name: /^Lighthouse chapters/ })).toBeVisible();
      await expect(list.getByText("Saved")).toBeVisible();
    });

    test("run threads sit in a folder: 'Morning summary · 3 runs' opens to the runs, each with its date", async ({ page }) => {
      await openChat(page, scheme, withFolder(), `?c=${MINE.id}`);
      const list = page.getByRole("navigation", { name: "Conversations" });
      const folder = list.getByRole("button", { name: /Morning summary · 3 runs/ });
      await expect(folder).toHaveAttribute("aria-expanded", "false");
      await folder.click();
      await expect(list.getByRole("button", { name: /^Morning summary · \d{1,2} \w{3} \d\d:\d\d/ })).toHaveCount(3);
      expect(await serious(page)).toEqual([]);
    });

    test("Stop leaves 'You stopped this answer' under the partial answer", async ({ page }) => {
      await openChat(page, scheme, stopped(), `?c=${MINE.id}`);
      await expect(page.getByText(/Chapter 1\. Mara climbed/)).toBeVisible();
      await expect(page.getByTestId("turn-stopped")).toHaveText("You stopped this answer");
      expect(await serious(page)).toEqual([]);
    });
  });

  test.describe(`${scheme}, 390 px`, () => {
    test.use({ viewport: { width: 390, height: 844 } });

    test("the phone's list has New chat, the folder and row menus without horizontal scroll", async ({ page }) => {
      await openChat(page, scheme, withFolder());
      await page.getByRole("button", { name: "Back to conversations" }).click();
      const list = page.getByRole("navigation", { name: "Conversations" });
      await expect(list.getByRole("button", { name: "New chat" })).toBeVisible();
      await list.getByRole("button", { name: /Morning summary · 3 runs/ }).click();
      await expect(list.getByRole("button", { name: /^Morning summary · \d{1,2} \w{3} \d\d:\d\d/ })).toHaveCount(3);
      expect(await overflow(page)).toBeLessThanOrEqual(0);
      expect(await serious(page)).toEqual([]);
    });

    test("Stop's mark fits at 390 px", async ({ page }) => {
      await openChat(page, scheme, stopped(), `?c=${MINE.id}`);
      await expect(page.getByTestId("turn-stopped")).toBeVisible();
      expect(await overflow(page)).toBeLessThanOrEqual(0);
    });
  });
}
