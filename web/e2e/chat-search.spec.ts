import AxeBuilder from "@axe-core/playwright";
import { expect, test, type Page } from "@playwright/test";

import { DEFAULT, MINE, TEAM, openChat, type Ev, type Fixture } from "./support/chat-mock";
import { overflow } from "./support/open";

// P5-CHAT-4 part B (D-229, C-727; docs/14 §Message search): the search box, chips, scope, results that open the message
// in place (P5-CHAT-2 jump), at 1280 px and 390 px in both themes; axe with no rule disabled.

const at = (m: number) => new Date(Date.now() - m * 60_000).toISOString();
const thread: Ev[] = [
  { seq: 1, type: "turn.queued", ts: at(300), payload: { turn_id: "t1", speaker_label: "Ada Lovelace", door: "dashboard", text: "First question about the release.", attachments: [] } },
  { seq: 2, type: "text", ts: at(299), payload: { turn_id: "t1", message_id: "m2", text: "The release is **ready** to ship." } },
  ...Array.from({ length: 24 }, (_, i): Ev => ({
    seq: 3 + i,
    type: i % 2 ? "text" : "turn.queued",
    ts: at(290 - i),
    payload: i % 2 ? { turn_id: `t${i}`, message_id: `m${i}`, text: `Filler answer number ${i} `.repeat(6) } : { turn_id: `t${i}`, speaker_label: "Ada Lovelace", door: "dashboard", text: `Filler question ${i}`, attachments: [] },
  })),
];

const hit = (conv: typeof MINE, seq: number, snippet: string, highlights: number[][], over: Record<string, unknown> = {}) => ({
  conversation: { id: conv.id, title: conv.title, kind: conv.kind, archived: false },
  agent: { id: conv.agent_id, name: "Daneel" },
  seq,
  type: "text",
  author: { kind: "agent", label: "Daneel" },
  at: at(299),
  snippet,
  highlights,
  ...over,
});

let requests: URL[] = [];
const fx = (over: Partial<Fixture> = {}): Fixture => ({
  ...DEFAULT,
  events: { [MINE.id]: thread, [TEAM.id]: thread.slice(0, 3) },
  search: (url) => {
    requests.push(url);
    return {
      items: [hit(MINE, 2, "The release is ready to ship.", [[4, 11]]), hit(TEAM, 1, "First question about the release.", [[27, 34]], { author: { kind: "person", label: "Ada Lovelace" }, type: "turn.queued" })],
      next_cursor: null,
      index_complete: false,
    };
  },
  ...over,
});

async function serious(page: Page) {
  const { violations } = await new AxeBuilder({ page }).analyze();
  return violations.filter((v) => v.impact === "serious" || v.impact === "critical").map((v) => `${v.id}: ${v.nodes.map((n) => n.target.join(" ")).slice(0, 3).join(" | ")}`);
}

const searchBox = (page: Page) => page.getByRole("combobox", { name: "Search conversations and messages" });
const list = (page: Page) => page.locator('nav[aria-label="Conversations"]');
async function search(page: Page, text: string) {
  const box = searchBox(page);
  await box.fill(text);
  await box.press("Enter");
  await expect(list(page).getByRole("link", { name: /GitHub triage room/ })).toBeVisible();
}

test.beforeEach(() => {
  requests = [];
});

for (const scheme of ["light", "dark"] as const) {
  test.describe(`${scheme}, 1280 px`, () => {
    test.use({ viewport: { width: 1280, height: 900 } });

    test("Enter searches; results widen the pane, show highlights and the indexing line; a result opens and highlights the message", async ({ page }) => {
      await openChat(page, scheme, fx());
      const before = (await list(page).boundingBox())!.width;
      await search(page, "release");
      expect((await list(page).boundingBox())!.width).toBeGreaterThan(before + 60);
      await expect(list(page).getByRole("status")).toHaveText("2 results");
      await expect(list(page).getByText("Older messages are still being indexed")).toBeVisible();
      await expect(list(page).locator("mark").first()).toHaveText("release");
      const q = requests.at(-1)!.searchParams;
      expect(q.get("q")).toBe("release");
      expect(q.get("agent_id")).not.toBeNull();
      expect(await serious(page)).toEqual([]);

      await list(page).getByRole("link", { name: /Direct chat/ }).click();
      await expect(page).toHaveURL(new RegExp(`c=${MINE.id}`));
      const msg = page.getByRole("log").locator('[data-seq="2"]');
      await expect(msg).toBeInViewport();
      await expect(list(page).getByRole("link", { name: /Direct chat/ })).toHaveAttribute("aria-current", "true");
      await expect.poll(() => new URL(page.url()).searchParams.has("m")).toBe(false);
      // the results stay until the search is cleared
      await expect(list(page).getByRole("status")).toHaveText("2 results");
      // a second result in another conversation opens too
      await list(page).getByRole("link", { name: /GitHub triage room/ }).click();
      await expect(page.getByRole("heading", { name: "GitHub triage room", level: 2 })).toBeVisible();
    });

    test("a typed filter becomes a chip, is sent as a parameter and is removable", async ({ page }) => {
      await openChat(page, scheme, fx());
      await searchBox(page).fill("release from:me has:file ");
      const chips = page.getByRole("group", { name: "Search filters" });
      await expect(chips).toContainText("From: me");
      await expect(chips).toContainText("Has: file");
      await searchBox(page).press("Enter");
      await expect(list(page).getByRole("status")).toBeVisible();
      const q = requests.at(-1)!.searchParams;
      expect([q.get("q"), q.get("author"), q.get("has")]).toEqual(["release", "me", "file"]);
      await page.getByRole("button", { name: "Remove filter From: me" }).click();
      await expect.poll(() => requests.at(-1)!.searchParams.has("author")).toBe(false);
      await expect(chips).not.toContainText("From: me");
    });

    test("in: offers conversation titles and the chosen one becomes conversation_id", async ({ page }) => {
      await openChat(page, scheme, fx());
      const box = searchBox(page);
      await box.fill("release in:GitHub");
      await expect(page.getByRole("option", { name: /GitHub triage room/ })).toBeVisible();
      await box.press("Enter");
      await expect(page.getByRole("group", { name: "Search filters" })).toContainText("In: GitHub triage room");
      await box.fill("release");
      await box.press("Enter");
      await expect(list(page).getByRole("status")).toBeVisible();
      expect(requests.at(-1)!.searchParams.get("conversation_id")).toBe(TEAM.id);
    });

    test("the scope switch searches All agents (no agent_id)", async ({ page }) => {
      await openChat(page, scheme, fx());
      await search(page, "release");
      await list(page).getByText("All agents").click();
      await expect.poll(() => requests.at(-1)!.searchParams.has("agent_id")).toBe(false);
      await list(page).getByRole("combobox", { name: "Sort results" }).selectOption("relevance");
      await expect.poll(() => requests.at(-1)!.searchParams.get("sort")).toBe("relevance");
    });

    test("/ and Ctrl+F focus the box from the panel; Esc clears the search", async ({ page }) => {
      await openChat(page, scheme, fx());
      await list(page).getByRole("tab", { name: "Archived" }).focus(); // focus inside the panel, not in a text field
      await page.keyboard.press("/");
      await expect(searchBox(page)).toBeFocused();
      await searchBox(page).fill("release");
      await page.locator("#chat-input").focus();
      await page.keyboard.type("a/b");
      await expect(page.locator("#chat-input")).toHaveValue(/a\/b/);
      await page.keyboard.press("Control+f");
      await expect(searchBox(page)).toBeFocused();
      await searchBox(page).press("Enter");
      await expect(list(page).getByRole("link", { name: /GitHub triage room/ })).toBeVisible();
      await searchBox(page).press("Escape");
      await expect(list(page).getByRole("link", { name: /GitHub triage room/ })).toHaveCount(0);
      await expect(list(page).getByRole("tab", { name: "Active" })).toBeVisible();
    });

    test("no results and a 422 are said under the box", async ({ page }) => {
      await openChat(page, scheme, fx({ search: () => ({ items: [], next_cursor: null, index_complete: true }) }));
      const box = searchBox(page);
      await box.fill("zzz");
      await box.press("Enter");
      await expect(page.getByText("No messages match “zzz”.")).toBeVisible();
      await expect(page.getByText(/Try fewer words/)).toBeVisible();
    });

    test("the suggestion list is accessible with the box focused", async ({ page }) => {
      await openChat(page, scheme, fx());
      await searchBox(page).fill("rel");
      await expect(page.getByRole("option", { name: /Search messages for “rel”/ })).toBeVisible();
      expect(await serious(page)).toEqual([]);
    });
  });
}

test.describe("390 px", () => {
  test.use({ viewport: { width: 390, height: 844 } });
  test("results replace the list, opening one shows the thread, Back returns to the results", async ({ page }) => {
    await openChat(page, "light", fx());
    await search(page, "release");
    expect(await overflow(page)).toBeLessThanOrEqual(0);
    expect(await serious(page)).toEqual([]);
    await list(page).getByRole("link", { name: /Direct chat/ }).click();
    await expect(page.getByRole("log")).toBeVisible();
    await expect(page.getByRole("log").locator('[data-seq="2"]')).toBeInViewport();
    await page.getByRole("button", { name: /Back/ }).first().click();
    await expect(list(page).getByRole("link", { name: /GitHub triage room/ })).toBeVisible();
    expect(await overflow(page)).toBeLessThanOrEqual(0);
  });
});
