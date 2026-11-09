import AxeBuilder from "@axe-core/playwright";
import { expect, test, type Page } from "@playwright/test";
import { mkdirSync } from "node:fs";

import {
  ANN_DIRECT, DEFAULT, ERRORED, MINE, NIGHTLY, OLD, RUNNING, SENTRY, TEAM, openChat,
  type Ev, type Fixture,
} from "./support/chat-mock";
import { overflow } from "./support/open";

// P2-UI-5 (docs/14 §Visual design › Agent chat, D-102): the messaging layout. Every state is opened in both
// themes at 1280 px and 390 px, screenshotted (outside the repo) and checked with axe (no rule disabled, nothing
// excluded). The layout assertions: the page does not scroll and the composer does not move at 1280 px; at 390 px
// there is no horizontal scroll, the list and the thread are two views, and Back (button and browser) returns to the list.

const SHOTS = "/tmp/aurora-screens/p2-ui-5";
mkdirSync(SHOTS, { recursive: true });

async function serious(page: Page) {
  const { violations } = await new AxeBuilder({ page }).analyze();
  return violations
    .filter((v) => v.impact === "serious" || v.impact === "critical")
    .map((v) => `${v.id} (${v.impact}): ${v.nodes.map((n) => n.target.join(" ")).slice(0, 5).join(" | ")}`);
}

const working = { ...MINE, active_turn: { id: "t1" } };
const fixtures: Record<string, Fixture> = {
  running: { ...DEFAULT, direct: working as never, active: [working as never, NIGHTLY, SENTRY, TEAM, ANN_DIRECT], events: { [MINE.id]: RUNNING } },
  empty: { ...DEFAULT, events: {} },
  error: { ...DEFAULT, events: { [MINE.id]: ERRORED } },
  owner: {
    ...DEFAULT,
    events: {
      [ANN_DIRECT.id]: [
        { seq: 1, type: "turn.queued", ts: new Date(Date.now() - 3 * 3_600_000).toISOString(), payload: { turn_id: "t1", speaker_label: "Grace Hopper", door: "dashboard", text: "Which Sentry issues are new since Monday?" } },
        { seq: 2, type: "text", ts: new Date(Date.now() - 3 * 3_600_000 + 60_000).toISOString(), payload: { turn_id: "t1", message_id: "m2", text: "Two: a timeout in `checkout` and a null deref in the importer." } },
      ] satisfies Ev[],
    },
  },
  archived: { ...DEFAULT, events: { [OLD.id]: [{ seq: 1, type: "turn.queued", ts: OLD.last_activity_at, payload: { turn_id: "t1", speaker_label: "Ada Lovelace", door: "dashboard", text: "Review the Notion handbook." } },
    { seq: 2, type: "text", ts: OLD.last_activity_at, payload: { turn_id: "t1", message_id: "m2", text: "The handbook is up to date." } },
    { seq: 3, type: "turn.finished", ts: OLD.last_activity_at, payload: { turn_id: "t1", status: "done" } }] } },
};

type Step = (page: Page, narrow: boolean) => Promise<void>;
const states: { name: string; fx: Fixture; query: (narrow: boolean) => string; prepare?: Step; phoneOnly?: boolean }[] = [
  {
    name: "running-held-approval",
    fx: fixtures.running,
    query: () => `?c=${MINE.id}`,
    prepare: async (page) => {
      await page.evaluate(() =>
        (window as unknown as { __es: { emit: (t: string, e: unknown) => void } }).__es.emit("activity", {
          seq: null, type: "activity", ts: new Date().toISOString(), payload: { turn_id: "t1", line: "Waiting for approval" },
        }),
      );
      await expect(page.getByRole("status").filter({ hasText: "Waiting for approval" })).toBeVisible();
      await expect(page.getByText("Held", { exact: true })).toBeVisible();
    },
  },
  {
    name: "archived-actions-menu",
    fx: fixtures.archived,
    query: () => "",
    prepare: async (page) => {
      await page.getByRole("tab", { name: "Archived" }).click();
      await page.getByRole("button", { name: /Notion handbook review/ }).click();
      await page.getByRole("button", { name: "Conversation actions" }).click();
      await expect(page.getByRole("menuitem", { name: "Unarchive" })).toBeVisible();
    },
  },
  { name: "empty-conversation", fx: fixtures.empty, query: () => `?c=${MINE.id}`, prepare: async (page) => { await expect(page.getByText("Start a conversation with Daneel")).toBeVisible(); } },
  { name: "owner-read-only", fx: fixtures.owner, query: () => `?c=${ANN_DIRECT.id}`, prepare: async (page) => { await expect(page.getByText(/Read-only view/)).toBeVisible(); } },
  { name: "error-block", fx: fixtures.error, query: () => `?c=${MINE.id}`, prepare: async (page) => { await expect(page.getByRole("link", { name: "Open LLM keys" })).toBeVisible(); } },
  { name: "phone-list", fx: DEFAULT, query: () => "", phoneOnly: true },
  {
    name: "phone-thread",
    fx: DEFAULT,
    query: () => "",
    phoneOnly: true,
    prepare: async (page) => {
      await page.getByRole("button", { name: /Direct chat/ }).first().click();
      await expect(page.getByRole("button", { name: "Back to conversations" })).toBeVisible();
    },
  },
];

for (const scheme of ["light", "dark"] as const) {
  for (const width of [1280, 390]) {
    test.describe(`states, ${scheme} theme, ${width} px`, () => {
      test.use({ viewport: { width, height: width === 1280 ? 900 : 844 } });
      for (const s of states) {
        if (s.phoneOnly && width !== 390) continue;
        test(s.name, async ({ page }) => {
          await openChat(page, scheme, s.fx, s.query(width === 390));
          await s.prepare?.(page, width === 390);
          expect(await overflow(page), "horizontal page scroll").toBeLessThanOrEqual(0);
          await page.screenshot({ path: `${SHOTS}/${s.name}-${scheme}-${width}.png` });
          expect(await serious(page)).toEqual([]);
        });
      }
    });
  }
}

// Bubble widths (owner, 2026-11-09; docs/14 › Agent chat): both parties at most 80 % of the thread column, and a
// short message stays on one line. Regression: person rows shrank to fit, so 78 % of the text's own width wrapped
// "hi" into "h / i".
const LONG = "The GitHub pull request, the Linear issue and the Sentry alert all point at the same importer timeout, so I would fix the retry budget first and then re-run the nightly job to confirm.";
const widths: Fixture = {
  ...DEFAULT,
  events: {
    [MINE.id]: [
      { seq: 1, type: "turn.queued", ts: new Date(Date.now() - 600_000).toISOString(), payload: { turn_id: "t1", speaker_label: "Ada Lovelace", door: "dashboard", text: "hello there", attachments: [] } },
      { seq: 2, type: "text", ts: new Date(Date.now() - 590_000).toISOString(), payload: { turn_id: "t1", message_id: "m2", text: "ok" } },
      { seq: 3, type: "turn.finished", ts: new Date(Date.now() - 590_000).toISOString(), payload: { turn_id: "t1", status: "done" } },
      { seq: 4, type: "turn.queued", ts: new Date(Date.now() - 300_000).toISOString(), payload: { turn_id: "t2", speaker_label: "Ada Lovelace", door: "dashboard", text: "hi", attachments: [] } },
      { seq: 5, type: "turn.queued", ts: new Date(Date.now() - 299_000).toISOString(), payload: { turn_id: "t3", speaker_label: "Ada Lovelace", door: "dashboard", text: LONG, attachments: [] } },
      { seq: 6, type: "text", ts: new Date(Date.now() - 290_000).toISOString(), payload: { turn_id: "t3", message_id: "m6", text: LONG } },
      { seq: 7, type: "turn.finished", ts: new Date(Date.now() - 290_000).toISOString(), payload: { turn_id: "t3", status: "done" } },
    ] satisfies Ev[],
  },
};

for (const width of [1280, 390]) {
  test.describe(`bubble widths, ${width} px`, () => {
    test.use({ viewport: { width, height: width === 1280 ? 900 : 844 } });
    test("short messages stay on one line; long ones use at most 80 % of the thread column", async ({ page }) => {
      await openChat(page, "light", widths, `?c=${MINE.id}`);
      const log = page.getByRole("log");
      const col = (await log.boundingBox())!.width;
      const bubble = (text: string, nth = 0) =>
        log.getByText(text, { exact: true }).nth(nth).locator("xpath=ancestor::div[contains(@class,'rounded-card')][1]");
      for (const short of ["hello there", "hi", "ok"]) {
        const text = log.getByText(short, { exact: true });
        const lineHeight = await text.evaluate((el) => parseFloat(getComputedStyle(el).lineHeight));
        expect((await text.boundingBox())!.height, `"${short}" wraps`).toBeLessThan(lineHeight * 1.5);
      }
      for (const nth of [0, 1]) {
        const w = (await bubble(LONG, nth).boundingBox())!.width;
        expect(w, `long bubble ${nth} wider than 80 %`).toBeLessThanOrEqual(col * 0.8 + 1);
        // The agent bubble is 80 % of its column (the thread minus the avatar), so it is a bit narrower.
        expect(w, `long bubble ${nth} does not use the room`).toBeGreaterThan(col * 0.6);
      }
      expect(await overflow(page), "horizontal page scroll").toBeLessThanOrEqual(0);
      await page.screenshot({ path: `${SHOTS}/bubble-widths-${width}.png` });
    });
  });
}

// (800 px tall: the Chat tab's compact agent header, tabs and page padding leave the panel at least 560 px.)
test.describe("layout, 1280 px", () => {
  test.use({ viewport: { width: 1280, height: 800 } });

  test("two panes; the page does not scroll and the composer does not move while the transcript grows", async ({ page }) => {
    await openChat(page, "light", DEFAULT);
    await expect(page.getByRole("navigation", { name: "Conversations" })).toBeVisible();
    await expect(page.getByRole("log")).toBeVisible();
    const composer = page.getByLabel("Message", { exact: true });
    const before = await composer.boundingBox();
    expect(before).not.toBeNull();
    const fits = () => page.evaluate(() => document.scrollingElement!.scrollHeight <= window.innerHeight);
    const excess = () => page.evaluate(() => document.scrollingElement!.scrollHeight - window.innerHeight);
    expect(await excess(), "the page scrolls before (px beyond the viewport)").toBeLessThanOrEqual(0);

    await page.evaluate(() => {
      const es = (window as unknown as { __es: { emit: (t: string, e: unknown) => void } }).__es;
      for (let i = 0; i < 30; i++) {
        es.emit("text", {
          seq: 100 + i, type: "text", ts: new Date().toISOString(),
          payload: { turn_id: "t9", message_id: `inj${i}`, text: `Injected message ${i}: the GitHub pull request and the Linear issue are both up to date, nothing to do.` },
        });
      }
    });
    await expect(page.getByText("Injected message 29", { exact: false })).toBeVisible();
    const log = await page.getByRole("log").evaluate((el) => ({ scroll: el.scrollHeight, client: el.clientHeight }));
    expect(log.scroll, "the transcript is longer than the panel").toBeGreaterThan(log.client);
    expect(await fits(), "page scrolls after").toBe(true);
    const after = await composer.boundingBox();
    expect(after).toEqual(before);
  });

  for (const scheme of ["light", "dark"] as const) {
    test(`P2-UI-5b: compact Chat header at 1280x800, long transcript, ${scheme} (screenshots)`, async ({ page }) => {
      const dir = "/tmp/aurora-screens/p2-ui-5b";
      mkdirSync(dir, { recursive: true });
      await openChat(page, scheme, DEFAULT);
      await page.evaluate(() => {
        const es = (window as unknown as { __es: { emit: (t: string, e: unknown) => void } }).__es;
        for (let i = 0; i < 30; i++) {
          es.emit("text", {
            seq: 100 + i, type: "text", ts: new Date().toISOString(),
            payload: { turn_id: "t9", message_id: `inj${i}`, text: `Injected message ${i}: the GitHub pull request and the Linear issue are both up to date, nothing to do.` },
          });
        }
      });
      await expect(page.getByText("Injected message 29", { exact: false })).toBeVisible();
      expect(await page.evaluate(() => document.scrollingElement!.scrollHeight - window.innerHeight), "page scrolls").toBeLessThanOrEqual(0);
      await page.screenshot({ path: `${dir}/chat-${scheme}-1280x800.png` });
      expect(await serious(page)).toEqual([]);
      // Another tab keeps the full header.
      await page.getByRole("tab", { name: "Instructions" }).click();
      await expect(page.getByRole("button", { name: "Freeze agent" })).toBeVisible();
      await page.screenshot({ path: `${dir}/instructions-${scheme}-1280x800.png` });
    });
  }

  test("the panel is at least 560 px tall and the list is 272-320 px wide", async ({ page }) => {
    await openChat(page, "light", DEFAULT);
    const nav = page.getByRole("navigation", { name: "Conversations" });
    const list = await nav.boundingBox();
    expect(list!.width).toBeGreaterThanOrEqual(272 - 1);
    expect(list!.width).toBeLessThanOrEqual(320 + 1);
    const panel = await nav.locator("xpath=../..").boundingBox(); // nav > pane > panel
    expect(panel!.height).toBeGreaterThanOrEqual(560);
  });

  test("the message toolbar shows on keyboard focus and the actions menu works with the keyboard", async ({ page }) => {
    await openChat(page, "light", DEFAULT);
    const toolbar = page.getByRole("toolbar", { name: "Message actions" }).first();
    await expect(toolbar).toHaveCSS("opacity", "0");
    await toolbar.getByRole("button", { name: "More message actions" }).focus();
    await expect(toolbar).toHaveCSS("opacity", "1");
    await page.keyboard.press("Enter");
    await expect(page.getByRole("menuitem", { name: "Redact…" })).toBeVisible();
    await page.keyboard.press("Escape");

    const actions = page.getByRole("button", { name: "Conversation actions" });
    await actions.focus();
    await page.keyboard.press("Enter");
    await expect(page.getByRole("menuitem", { name: "Archive" })).toBeFocused();
    await page.keyboard.press("ArrowDown");
    await expect(page.getByRole("menuitem", { name: "Export JSON" })).toBeFocused();
    await expect(page.getByRole("menuitem", { name: "Export with files (zip)" })).toBeVisible();
    await expect(page.getByRole("menuitem", { name: "Delete conversation…" })).toBeVisible();
  });

  test("tabbing into a message reveals the toolbar with its first button focused; no focused control is ever invisible", async ({ page }) => {
    await openChat(page, "light", DEFAULT);
    const toolbar = page.getByRole("toolbar", { name: "Message actions" }).first();
    await expect(toolbar).toHaveCSS("opacity", "0");
    const invisibleFocus = () =>
      page.evaluate(() => {
        for (let el: Element | null = document.activeElement; el && el !== document.body; el = el.parentElement) {
          const cs = getComputedStyle(el);
          if (Number(cs.opacity) === 0 || cs.visibility === "hidden") return (document.activeElement as HTMLElement).outerHTML.slice(0, 80);
        }
        return null;
      });
    await page.getByLabel("Message", { exact: true }).focus();
    const first = toolbar.getByRole("button").first();
    let reached = false;
    for (let i = 0; i < 60; i++) {
      await page.keyboard.press("Shift+Tab"); // from the composer back through the transcript
      expect(await invisibleFocus(), `Shift+Tab ${i + 1} landed on an invisible control`).toBeNull();
      if (await toolbar.getByRole("button").first().evaluate((el) => el === document.activeElement)) {
        reached = true;
        break;
      }
    }
    expect(reached, "the first message's first toolbar button was reached by keyboard").toBe(true);
    await expect(toolbar).toHaveCSS("opacity", "1");
    await expect(first).toBeFocused();
  });

  test("Live turns into Reconnecting… and back from the stream state", async ({ page }) => {
    await openChat(page, "light", DEFAULT);
    await expect(page.getByText("Live", { exact: true })).toBeVisible();
    await page.evaluate(() => (window as unknown as { __es: { down: () => void } }).__es.down());
    await expect(page.getByText("Reconnecting…")).toBeVisible();
    await page.evaluate(() => (window as unknown as { __es: { up: () => void } }).__es.up());
    await expect(page.getByText("Live", { exact: true })).toBeVisible();
  });

  test("a deep link opens the conversation and the selection is in the URL", async ({ page }) => {
    await openChat(page, "light", DEFAULT, `?c=${SENTRY.id}`);
    await expect(page.getByRole("heading", { name: "Sentry alerts", level: 2 })).toBeVisible();
    await page.getByRole("button", { name: /Weekly Linear digest/ }).click();
    await expect(page).toHaveURL(new RegExp(`c=${NIGHTLY.id}`));
    await page.goBack();
    await expect(page.getByRole("heading", { name: "Sentry alerts", level: 2 })).toBeVisible();
    await page.getByRole("searchbox", { name: "Filter conversations" }).fill("handbook");
    await expect(page.getByRole("button", { name: /Sentry alerts/ })).toHaveCount(0);
  });
});

test.describe("layout, 390 px", () => {
  test.use({ viewport: { width: 390, height: 844 } });

  test("the list and the thread are two views; Back and the browser's back return to the list", async ({ page }) => {
    await openChat(page, "light", DEFAULT);
    const list = page.getByRole("navigation", { name: "Conversations" });
    await expect(list).toBeVisible();
    await expect(page.getByRole("log")).toBeHidden();
    expect(await overflow(page)).toBeLessThanOrEqual(0);

    await page.getByRole("button", { name: /Weekly Linear digest/ }).click();
    await expect(page.getByRole("log")).toBeVisible();
    await expect(list).toBeHidden();
    expect(await overflow(page)).toBeLessThanOrEqual(0);

    await page.getByRole("button", { name: "Back to conversations" }).click();
    await expect(list).toBeVisible();
    expect(new URL(page.url()).searchParams.has("c")).toBe(false);

    await page.getByRole("button", { name: /Weekly Linear digest/ }).click();
    await expect(page.getByRole("log")).toBeVisible();
    await page.goBack();
    await expect(list).toBeVisible();
    await expect(page.getByRole("log")).toBeHidden();
  });

  test("the composer is one row of 44 px controls and every thread control is at least 44 px", async ({ page }) => {
    await openChat(page, "light", { ...DEFAULT, direct: working as never, active: [working as never, ...DEFAULT.active.slice(1)] }, `?c=${MINE.id}`);
    const box = async (name: string | RegExp, role: "button" | "textbox" = "button") => {
      const b = await page.getByRole(role, { name }).first().boundingBox();
      expect(b, String(name)).not.toBeNull();
      return b!;
    };
    const attach = await page.getByText("Attach", { exact: true }).locator("xpath=ancestor::label").boundingBox();
    const send = await box("Follow up");
    const stop = await box("Stop");
    const text = await box("Message", "textbox");
    for (const [name, b] of Object.entries({ attach, send, stop })) {
      expect(b!.width, `${name} width`).toBeGreaterThanOrEqual(43.5);
      expect(b!.height, `${name} height`).toBeGreaterThanOrEqual(43.5);
    }
    expect(text.height).toBeGreaterThanOrEqual(43.5);
    const centres = [attach!, text, stop, send].map((b) => b.y + b.height / 2);
    expect(Math.max(...centres) - Math.min(...centres), "one row").toBeLessThanOrEqual(8);
    expect(await box("Back to conversations").then((b) => b.height)).toBeGreaterThanOrEqual(43.5);
    expect(await box("Conversation actions").then((b) => b.height)).toBeGreaterThanOrEqual(43.5);
    await expect(page.getByText("Enter to send")).toBeHidden();
  });

  test("inline approval buttons are a two-column grid of 44 px buttons", async ({ page }) => {
    await openChat(page, "light", fixtures.running, `?c=${MINE.id}`);
    const approve = await page.getByRole("button", { name: "Approve" }).boundingBox();
    const deny = await page.getByRole("button", { name: "Deny" }).boundingBox();
    expect(approve!.height).toBeGreaterThanOrEqual(43.5);
    expect(deny!.height).toBeGreaterThanOrEqual(43.5);
    expect(Math.abs(approve!.y - deny!.y)).toBeLessThanOrEqual(1);
    void TEAM;
  });
});
