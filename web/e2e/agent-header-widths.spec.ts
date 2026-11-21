import { expect, test, type Locator, type Page } from "@playwright/test";

import { AGENT_ROUTE, ANN_DIRECT, DEFAULT, MINE, conv, mockChat } from "./support/chat-mock";
import { id, mockApi, user } from "./support/mock-api";
import { overflow } from "./support/open";

// P5-REFINE-20 batch 9 (access-3 at 390, the agent header at tablet widths): at every width from 390 up the agent's name is
// visible (h1 wider than 0), its state ("Can't answer: no AI key", the longest one) shows whole and is covered by nothing (not
// Status, not Settings, not the section tabs), the page never scrolls sideways, and the give-key card's button wraps inside
// its card. Mocked API, both themes.

const GIVE = { action: "give_key", key: { secret_id: id(60), name: "Company key", scope: "organization" } };

async function open(page: Page, scheme: "light" | "dark", query = "") {
  await mockApi(page);
  await mockChat(page, { ...DEFAULT, events: { [MINE.id]: [] } });
  await page.route("**/v1/**", (route) => {
    const req = route.request();
    if (req.method() === "GET" && /^\/v1\/agents\/[^/]+\/answer-readiness$/.test(new URL(req.url()).pathname)) {
      return route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ can_answer: false, reason: "no_ai_key", usable_keys: 0, fix: GIVE }),
      });
    }
    return route.fallback();
  });
  await page.emulateMedia({ colorScheme: scheme });
  await page.goto(`${AGENT_ROUTE}${query}`);
  if (!query) await page.getByRole("tab", { name: "Chat" }).click();
  await page.waitForLoadState("networkidle");
}

const box = async (l: Locator) => (await l.boundingBox())!;
const overlaps = (a: { x: number; y: number; width: number; height: number }, b: typeof a) =>
  a.x < b.x + b.width - 1 && b.x < a.x + a.width - 1 && a.y < b.y + b.height - 1 && b.y < a.y + a.height - 1;

// P5-REFINE-20 batch 10: the Apps & access tab's starter-key card ("Daneel has no AI key") keeps its button in the card at
// 768 and 790 too, and the Chat tab with a thread open (first answer done, so the callout shows) keeps the composer, Send,
// the callout's buttons and the Live badge inside their boxes from 768 up: the list sits beside the thread only from 1024.
const at = (agoMs: number) => new Date(Date.now() - agoMs).toISOString();
const ANSWERED = [
  { seq: 1, type: "turn.queued", ts: at(120_000), payload: { turn_id: "t1", speaker_label: "Ada Lovelace", door: "dashboard", text: "Hello", attachments: [] } },
  { seq: 2, type: "text", ts: at(110_000), payload: { turn_id: "t1", message_id: "m2", text: "Hi, I can help." } },
  { seq: 3, type: "turn.finished", ts: at(100_000), payload: { turn_id: "t1", status: "done", ai_key: "organization" } },
];
const inside = (inner: { x: number; y: number; width: number; height: number }, outer: typeof inner) =>
  inner.x >= outer.x - 1 && inner.x + inner.width <= outer.x + outer.width + 1;

for (const scheme of ["light", "dark"] as const) {
  for (const width of [390, 768, 790, 820, 900, 1024]) {
    test.describe(`access tab card, ${scheme} theme, ${width} px`, () => {
      test.use({ viewport: { width, height: 900 } });

      test("the give-key button stays in its card and the page does not scroll sideways", async ({ page }) => {
        await open(page, scheme, "?tab=access");
        const card = page.getByTestId("starter-key-card");
        const give = card.getByRole("button", { name: "Give Daneel the Company key" });
        await expect(give).toBeVisible();
        expect(inside(await box(give), await box(card)), "the button is inside its card").toBe(true);
        expect(await card.evaluate((e) => e.scrollWidth - e.clientWidth), "the card does not scroll").toBeLessThanOrEqual(0);
        expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBe(await page.evaluate(() => document.documentElement.clientWidth));
      });
    });
  }

  for (const width of [390, 768, 820, 900, 1024]) {
    test.describe(`chat thread, ${scheme} theme, ${width} px`, () => {
      test.use({ viewport: { width, height: 900 } });

      test("the composer, Send, the callout and the Live badge fit; the back arrow shows below 1024", async ({ page }) => {
        await mockApi(page);
        await mockChat(page, { ...DEFAULT, events: { [MINE.id]: ANSWERED } });
        await page.emulateMedia({ colorScheme: scheme });
        await page.goto(`${AGENT_ROUTE}?c=${MINE.id}`);
        await page.getByRole("tab", { name: "Chat" }).click();
        const thread = page.getByRole("region", { name: "Chat" });
        const send = thread.getByRole("button", { name: "Send", exact: true });
        await expect(send).toBeVisible();
        const callout = page.getByTestId("first-success-first-answer");
        await expect(callout).toBeVisible();
        const composer = page.getByTestId("composer-hint").locator("xpath=ancestor::div[contains(@class,'border-t')][1]");
        const cBox = await box(composer);
        expect(inside(await box(send), cBox), "Send is inside the composer").toBe(true);
        const calloutBox = await box(callout);
        for (const b of await callout.getByRole("button").all()) {
          const bb = await box(b);
          expect(inside(bb, calloutBox), "the callout's buttons are inside it").toBe(true);
          expect(overlaps(bb, await box(callout.locator("p"))), "the callout's buttons are off its sentence").toBe(false);
        }
        const header = thread.locator("header").first();
        const live = header.getByText("Live", { exact: true });
        await expect(live).toBeVisible();
        const lb = await box(live);
        expect(inside(lb, await box(header)), "the badge is inside the header").toBe(true);
        const privacy = header.locator("p").last();
        expect(overlaps(lb, await box(privacy)), "the badge is off the privacy line").toBe(false);
        const back = page.getByRole("button", { name: "Back to conversations" });
        if (width < 1024) await expect(back).toBeVisible();
        else await expect(back).toBeHidden();
        expect(await overflow(page), "horizontal page scroll").toBeLessThanOrEqual(0);
        expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBe(await page.evaluate(() => document.documentElement.clientWidth));
      });
    });
  }
}

for (const scheme of ["light", "dark"] as const) {
  for (const width of [390, 768, 820, 900, 1024, 1280]) {
    test.describe(`${scheme} theme, ${width} px`, () => {
      test.use({ viewport: { width, height: 900 } });

      test("the agent header keeps the name and the whole state visible, with no sideways scroll", async ({ page }) => {
        await open(page, scheme);
        const header = page.locator("header").filter({ has: page.getByTestId("agent-state") });
        const h1 = header.getByRole("heading", { name: "Daneel", level: 1 });
        const state = page.getByTestId("agent-state");
        await expect(state).toContainText("Can't answer: no AI key");
        expect((await box(h1)).width, "the name is not squeezed out").toBeGreaterThan(40);
        // The h1 shows all of its text (not truncated to an ellipsis).
        expect(await h1.evaluate((e) => e.scrollWidth - e.clientWidth), "the name is not clipped").toBeLessThanOrEqual(0);
        // The state's text is whole: its own box is not clipped and no button or tab covers it.
        const text = state.locator("span[aria-live]");
        expect(await text.evaluate((e) => e.scrollWidth - e.clientWidth), "the state text is not clipped").toBeLessThanOrEqual(0);
        const stateBox = await box(text);
        const covers = [header.getByRole("button", { name: "Settings" }), header.getByRole("tablist")];
        if (width < 768) covers.push(header.getByRole("button", { name: "Status" }));
        for (const c of covers) expect(overlaps(stateBox, await box(c)), "the state is covered").toBe(false);
        // The state's own words are what is on top at their corners.
        for (const [px, py] of [
          [stateBox.x + 2, stateBox.y + 2],
          [stateBox.x + stateBox.width - 2, stateBox.y + stateBox.height - 2],
        ]) {
          expect(await page.evaluate(([x, y]) => !!document.elementFromPoint(x, y)?.closest("[data-testid=agent-state]"), [px, py]), "the state is on top").toBe(true);
        }
        expect(await overflow(page), "horizontal page scroll").toBeLessThanOrEqual(0);
        // The card's (and the header row's) give-key button stays inside what holds it.
        const give = page.getByRole("region", { name: "Daneel has no AI model yet" }).getByRole("button", { name: "Give Daneel the Company key" });
        await expect(give).toBeVisible();
        const card = await box(page.getByRole("region", { name: "Daneel has no AI model yet" }));
        const gb = await box(give);
        expect(gb.x + gb.width, "the card's button stays in its card").toBeLessThanOrEqual(card.x + card.width + 1);
        const docWidth = await page.evaluate(() => document.documentElement.scrollWidth);
        const clientWidth = await page.evaluate(() => document.documentElement.clientWidth);
        expect(docWidth).toBe(clientWidth);
      });
    });
  }
}

// P5-REFINE-20 batch 11: the chat card at 768–1023 px. The thread header's lines (title, "Conversations", the privacy line) all
// sit inside the 64 px header and the card, its bottom border level with the status rail's; the list view keeps the status rail
// inside the card (focusing "Expand status panel" does not scroll the card sideways); the give-key card's dot stays beside the
// sentence's first line.
const within = (inner: { x: number; y: number; width: number; height: number }, outer: typeof inner) =>
  inner.x >= outer.x - 1 && inner.y >= outer.y - 1 && inner.x + inner.width <= outer.x + outer.width + 1 && inner.y + inner.height <= outer.y + outer.height + 1;
const card = (l: Locator) => l.locator("xpath=ancestor::div[contains(@class,'rounded-card')][1]");

for (const scheme of ["light", "dark"] as const) {
  for (const width of [768, 820, 900, 960, 1023]) {
    test.describe(`chat card, ${scheme} theme, ${width} px`, () => {
      test.use({ viewport: { width, height: 900 } });

      test("every header line is inside the header and the card, level with the rail's border", async ({ page }) => {
        await mockApi(page);
        await mockChat(page, { ...DEFAULT, events: { [MINE.id]: ANSWERED } });
        await page.emulateMedia({ colorScheme: scheme });
        await page.goto(`${AGENT_ROUTE}?c=${MINE.id}`);
        await page.getByRole("tab", { name: "Chat" }).click();
        const thread = page.getByRole("region", { name: "Chat" });
        const header = thread.locator("header").first();
        await expect(header.getByRole("heading", { level: 2 })).toBeVisible();
        const hb = await box(header);
        const cb = await box(card(header));
        expect(within(hb, cb), "the header is inside the card").toBe(true);
        for (const line of await header.locator("h2, p").all()) {
          if (!(await line.isVisible())) continue;
          const lb = await box(line);
          expect(within(lb, hb), `"${await line.innerText()}" is inside the header`).toBe(true);
        }
        const railHeader = page.getByRole("complementary", { name: "Status" }).locator("xpath=./div[1]");
        const rb = await box(railHeader);
        // (batch 12) Below 1280 the meta line is its own row under the 64 px title row: that row's top border is level with the rail's.
        const metaTop = (await box(header.getByTestId("thread-meta"))).y + 1;
        expect(Math.abs(rb.y + rb.height - metaTop), "the title row's border is level with the rail's").toBeLessThanOrEqual(1);
        expect(await overflow(page), "horizontal page scroll").toBeLessThanOrEqual(0);
      });

      test("in the list view the status rail is inside the card and focusing Expand status panel does not scroll it", async ({ page }) => {
        await open(page, scheme);
        // The Chat tab opens the latest thread; the back arrow is the list view.
        await page.getByRole("button", { name: "Back to conversations" }).click();
        const nav = page.getByRole("navigation", { name: "Conversations" });
        await expect(nav).toBeVisible();
        const panel = card(nav);
        const rail = page.getByRole("complementary", { name: "Status" });
        await expect(rail).toBeVisible();
        expect(within(await box(rail), await box(panel)), "the rail is inside the card").toBe(true);
        expect(within(await box(nav), await box(panel)), "the list is inside the card").toBe(true);
        await rail.getByRole("button", { name: "Expand status panel" }).focus();
        expect(await panel.evaluate((e) => e.scrollLeft), "the card did not scroll").toBe(0);
        expect(await panel.evaluate((e) => e.scrollWidth - e.clientWidth), "the card has nothing to scroll").toBeLessThanOrEqual(0);
      });

      test("the give-key card's dot is beside the sentence's first line", async ({ page }) => {
        await open(page, scheme, "?tab=access");
        const starter = page.getByTestId("starter-key-card");
        const sentence = starter.locator("p").first();
        await expect(sentence).toBeVisible();
        const dot = starter.locator("span[aria-hidden]").first();
        const sb = await box(sentence);
        const db = await box(dot);
        expect(db.x + db.width, "the dot is left of the sentence").toBeLessThanOrEqual(sb.x + 1);
        expect(db.y, "the dot is level with the first line").toBeGreaterThanOrEqual(sb.y - 1);
        expect(db.y + db.height, "the dot is within the first line").toBeLessThanOrEqual(sb.y + 24);
      });
    });
  }
}

// P5-REFINE-20 batch 12: the thread header's meta line is readable without hover. Below 1280 it is its own full-width row under
// the title row, so the whole privacy note and "Archived <date> · read only" are in the DOM and visible (no title tooltip, no
// clipping) on an archived direct chat and on another member's chat, at every width; every line lies inside the header and the
// card, and nothing scrolls sideways.
const minutesAgo = (m: number) => new Date(Date.now() - m * 60_000).toISOString();
const ARCHIVED_DIRECT = conv(7, "direct", { direct_user_id: user.id, archived_at: minutesAgo(60 * 24 * 3), last_activity_at: minutesAgo(60 * 24 * 4) });
const META_CASES = [
  { name: "an archived direct chat", fx: { ...DEFAULT, archived: [ARCHIVED_DIRECT], events: { [ARCHIVED_DIRECT.id]: [] } }, conv: ARCHIVED_DIRECT.id, archived: true, texts: ["Only you and workspace owners can see this conversation", "Archived", "read only"] },
  { name: "another member's chat", fx: { ...DEFAULT, events: { [ANN_DIRECT.id]: [] } }, conv: ANN_DIRECT.id, archived: false, texts: ["Only Grace Hopper and workspace owners can see this conversation"] },
];

for (const scheme of ["light", "dark"] as const) {
  for (const width of [390, 768, 820, 900, 960, 1023, 1024, 1100, 1280]) {
    test.describe(`thread meta line, ${scheme} theme, ${width} px`, () => {
      test.use({ viewport: { width, height: 900 } });

      for (const c of META_CASES) {
        test(`${c.name}: the whole line is visible inside the header and the card`, async ({ page }) => {
          await mockApi(page);
          await mockChat(page, c.fx);
          await page.emulateMedia({ colorScheme: scheme });
          await page.goto(c.archived ? AGENT_ROUTE : `${AGENT_ROUTE}?c=${c.conv}`);
          await page.getByRole("tab", { name: "Chat" }).click();
          if (c.archived) {
            // An archived chat opens from the list's Archived tab (below 1024 the list is one tap back).
            if (width < 1024) await page.getByRole("button", { name: "Back to conversations" }).click();
            await page.getByRole("tab", { name: "Archived" }).click();
            await page.locator("#chat-list-panel-archived").getByRole("button").first().click();
          }
          const thread = page.getByRole("region", { name: "Chat" });
          const header = thread.locator("header").first();
          const meta = header.getByTestId("thread-meta");
          await expect(meta).toBeVisible();
          for (const t of c.texts) await expect(meta).toContainText(t);
          // Nothing lives only in a tooltip, and nothing is clipped.
          expect(await header.locator("[title]").count(), "no title tooltip carries text").toBe(0);
          expect(await meta.evaluate((e) => e.scrollWidth - e.clientWidth), "the meta line is not clipped sideways").toBeLessThanOrEqual(0);
          expect(await meta.evaluate((e) => e.scrollHeight - e.clientHeight), "the meta line is not clipped vertically").toBeLessThanOrEqual(0);
          const hb = await box(header);
          const cb = await box(card(header));
          expect(within(hb, cb), "the header is inside the card").toBe(true);
          if (width < 1280) {
            // (batch 13) The meta row's borders run edge to edge: both ends sit on the header's.
            const mb = await box(meta);
            expect(Math.abs(mb.x - hb.x), "the meta row starts at the header's left edge").toBeLessThanOrEqual(1);
            expect(Math.abs(mb.x + mb.width - (hb.x + hb.width)), "the meta row ends at the header's right edge").toBeLessThanOrEqual(1);
          }
          for (const line of await header.locator("h2, p, button").all()) {
            if (!(await line.isVisible())) continue;
            expect(within(await box(line), hb), `"${(await line.innerText()).slice(0, 40)}" is inside the header`).toBe(true);
          }
          expect(await header.evaluate((e) => e.scrollWidth - e.clientWidth), "the header does not scroll").toBeLessThanOrEqual(0);
          if (width >= 1280) expect(hb.height, "the header is the 64 px one").toBeLessThanOrEqual(64);
          expect(await overflow(page), "horizontal page scroll").toBeLessThanOrEqual(0);
          expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBe(await page.evaluate(() => document.documentElement.clientWidth));
        });
      }
    });
  }
}
