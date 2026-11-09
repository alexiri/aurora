import { readFileSync } from "node:fs";
import { join } from "node:path";
import { expect, type Page } from "@playwright/test";

import { agent, id, mockApi, now, org, user } from "./mock-api";

// Chat mock for the chat-v2 specs (P2-UI-5): the conversation endpoints plus a controllable EventSource, on top of
// the shared API mock. The data uses the REAL built-in packages (GitHub, Linear, Sentry, Notion: names from
// plugins/builtin/*/plugin.yaml; the attachment image is the GitHub package's own icon).

const BUILTIN = join(__dirname, "..", "..", "..", "plugins", "builtin");
export const AGENT_ROUTE = "/o/acme/w/main/agents/daneel";

export interface Ev {
  seq: number;
  type: string;
  ts: string;
  payload: Record<string, unknown>;
}

const minutesAgo = (m: number) => new Date(Date.now() - m * 60_000).toISOString();

export const conv = (n: number, kind: string, over: Record<string, unknown> = {}) => ({
  id: id(200 + n), agent_id: agent.id, kind, external_ref: `ref-${n}`, title: null, direct_user_id: null, direct_user: null, origin_conversation_id: null,
  last_seq: 0, app_turn_streak: 0, app_turn_capped: false, active_turn: null, last_activity_at: minutesAgo(30), archived_at: null, created_at: now, ...over,
});

export const MINE = conv(1, "direct", { direct_user_id: user.id, last_activity_at: minutesAgo(2) });
export const NIGHTLY = conv(2, "schedule", { title: "Weekly Linear digest", last_activity_at: minutesAgo(60 * 20) });
export const SENTRY = conv(3, "webhook", { title: "Sentry alerts", last_activity_at: minutesAgo(60 * 50) });
export const TEAM = conv(4, "group", { title: "GitHub triage room", last_activity_at: minutesAgo(60 * 24 * 9) });
export const ANN_DIRECT = conv(5, "direct", { direct_user_id: id(98), direct_user: { id: id(98), display_name: "Grace Hopper" }, last_activity_at: minutesAgo(60 * 3) });
export const OLD = conv(6, "group", { title: "Notion handbook review", archived_at: minutesAgo(60 * 24 * 3), last_activity_at: minutesAgo(60 * 24 * 4) });

const person = (seq: number, m: number, text: string, over: Record<string, unknown> = {}): Ev => ({
  seq, type: "turn.queued", ts: minutesAgo(m), payload: { turn_id: `t${seq}`, speaker_label: "Ada Lovelace", door: "dashboard", text, attachments: [], ...over },
});
const reply = (seq: number, m: number, turn: string, text: string): Ev => ({ seq, type: "text", ts: minutesAgo(m), payload: { turn_id: turn, message_id: `m${seq}`, text } });

/** A running turn with a held approval, a folded tool group, an attachment and a failed call. */
export const RUNNING: Ev[] = [
  person(1, 12, "Can you merge pull request #42 in acme/web once CI is green? Also summarise the open **Linear** issues for the release."),
  reply(2, 11, "t1", "Sure. I'll check the pull request and the issues first.\n\n- `acme/web#42` is green\n- 3 open Linear issues tagged `release`"),
  { seq: 3, type: "tool.started", ts: minutesAgo(10), payload: { turn_id: "t1", call_id: "c1", name: "github", summary: "GET /repos/acme/web/pulls/42" } },
  { seq: 4, type: "tool.finished", ts: new Date(Date.now() - 10 * 60_000 + 1200).toISOString(), payload: { turn_id: "t1", call_id: "c1", ok: true, summary: "200" } },
  { seq: 5, type: "tool.started", ts: minutesAgo(9), payload: { turn_id: "t1", call_id: "c2", name: "linear", summary: "POST /graphql issues(release)" } },
  { seq: 6, type: "tool.finished", ts: new Date(Date.now() - 9 * 60_000 + 3100).toISOString(), payload: { turn_id: "t1", call_id: "c2", ok: false, summary: "blocked by policy" } },
  { seq: 7, type: "attachment", ts: minutesAgo(8), payload: { turn_id: "t1", attachment_id: id(300), filename: "github-icon.png", content_type: "image/png", size: 5120 } },
  { seq: 8, type: "approval.pending", ts: minutesAgo(7), payload: { turn_id: "t1", approval_id: id(60), summary: "Merge pull request #42 in acme/web" } },
  { seq: 9, type: "message.joined", ts: minutesAgo(6), payload: { turn_id: "t1", followup_id: id(310) } },
];

export const ERRORED: Ev[] = [
  person(1, 5, "Summarise today's Sentry alerts."),
  { seq: 2, type: "error", ts: minutesAgo(4), payload: { turn_id: "t1", code: "no_model_key", message: "No usable LLM key: 1 granted key not usable (origin_unverified)." } },
  { seq: 3, type: "turn.finished", ts: minutesAgo(4), payload: { turn_id: "t1", status: "failed", error_code: "no_model_key" } },
];

/** FIX-021 (D-136): a usable key and no model chosen. */
export const NO_MODEL: Ev[] = [
  person(1, 5, "Summarise today's Sentry alerts."),
  { seq: 2, type: "error", ts: minutesAgo(4), payload: { turn_id: "t1", code: "no_default_model", message: "An LLM key is usable but no model is chosen for this agent and its endpoint has no default model." } },
  { seq: 3, type: "turn.finished", ts: minutesAgo(4), payload: { turn_id: "t1", status: "failed", error_code: "no_default_model" } },
];

export const WITH_REDACTED: Ev[] = [
  person(1, 60 * 30, "Here is the staging token: ..."),
  { seq: 2, type: "turn.queued", ts: minutesAgo(60 * 29), payload: { turn_id: "t2", redacted: true, redacted_by: "Ada Lovelace", reason: "pasted a secret" } },
  reply(3, 60 * 28, "t1", "Thanks, I have not stored it."),
  person(4, 3, "Thanks! Now check the Notion handbook page."),
  reply(5, 2, "t4", "Done: the handbook page is up to date."),
];

export interface Fixture {
  direct: ReturnType<typeof conv>;
  active: ReturnType<typeof conv>[];
  archived: ReturnType<typeof conv>[];
  events: Record<string, Ev[]>;
}

export const DEFAULT: Fixture = {
  direct: MINE,
  active: [MINE, NIGHTLY, SENTRY, TEAM, ANN_DIRECT],
  archived: [OLD],
  events: { [MINE.id]: WITH_REDACTED },
};

const ICON = readFileSync(join(BUILTIN, "github", "assets", "icon.png"));

/** A fake EventSource the spec drives: `window.__es.emit(type, event)`, `window.__es.down()` / `.up()`. */
const FAKE_EVENT_SOURCE = () => {
  type Listener = (e: { data: string; type: string }) => void;
  const all: FakeES[] = [];
  let mode: "up" | "down" = "up";
  class FakeES {
    static CONNECTING = 0;
    static OPEN = 1;
    static CLOSED = 2;
    readyState = 0;
    onopen: (() => void) | null = null;
    onerror: (() => void) | null = null;
    listeners: Record<string, Listener[]> = {};
    constructor(public url: string) {
      all.push(this);
      setTimeout(() => this.settle(), 0);
    }
    settle() {
      if (this.readyState === 2) return;
      if (mode === "up") {
        this.readyState = 1;
        this.onopen?.();
      } else {
        this.readyState = 0;
        this.onerror?.();
      }
    }
    addEventListener(t: string, fn: Listener) {
      (this.listeners[t] ??= []).push(fn);
    }
    removeEventListener() {}
    close() {
      this.readyState = 2;
    }
  }
  (window as unknown as { EventSource: unknown }).EventSource = FakeES;
  (window as unknown as { __es: unknown }).__es = {
    emit(type: string, ev: unknown) {
      for (const es of all) if (es.url.includes("/conversations/")) for (const fn of es.listeners[type] ?? []) fn({ type, data: JSON.stringify(ev) });
    },
    down() {
      mode = "down";
      for (const es of all) es.settle();
    },
    up() {
      mode = "up";
      for (const es of all) es.settle();
    },
  };
};

export async function mockChat(page: Page, fx: Fixture = DEFAULT) {
  await page.addInitScript(FAKE_EVENT_SOURCE);
  await page.route("**/v1/**", async (route) => {
    const url = new URL(route.request().url());
    const p = url.pathname;
    const method = route.request().method();
    const json = (b: unknown, status = 200) => route.fulfill({ status, contentType: "application/json", body: JSON.stringify(b) });
    if (method === "POST" && p === `/v1/agents/${agent.id}/conversations/direct`) return json(fx.direct);
    if (method === "GET" && p === `/v1/agents/${agent.id}/conversations`) {
      return json({ items: url.searchParams.get("archived") === "true" ? fx.archived : fx.active, next_cursor: null });
    }
    const events = p.match(/^\/v1\/conversations\/([^/]+)\/events$/);
    if (events && method === "GET") {
      const items = (fx.events[events[1]] ?? []).filter((e) => e.seq > Number(url.searchParams.get("after_seq") ?? 0));
      const all = fx.events[events[1]] ?? [];
      return json({ items, last_seq: all.length ? all[all.length - 1].seq : 0, purged_attachments: [] });
    }
    if (p === `/v1/orgs/${org.id}/retention`) return json({ conversations_days: 90, conversations_days_max: null });
    if (p.startsWith("/v1/attachments/") && p.endsWith("/content")) return route.fulfill({ status: 200, contentType: "image/png", body: ICON });
    if (method === "POST" && /^\/v1\/conversations\/[^/]+\/messages$/.test(p)) return json({ turn: { id: "t-new" }, followup: null }, 202);
    return route.fallback();
  });
}

/** Opens the agent's Chat tab with the chat mock, in `scheme`, and waits for the thread. */
export async function openChat(page: Page, scheme: "light" | "dark", fx: Fixture = DEFAULT, query = "") {
  const errors: string[] = [];
  page.on("pageerror", (e) => errors.push(e.message));
  await mockApi(page);
  await mockChat(page, fx);
  await page.emulateMedia({ colorScheme: scheme });
  await page.goto(`${AGENT_ROUTE}${query}`);
  await page.getByRole("tab", { name: "Chat" }).click();
  // (below 768 px the list is hidden while a thread is open: wait for it to exist, not to be visible)
  await page.locator('nav[aria-label="Conversations"]').waitFor({ state: "attached" });
  await page.waitForLoadState("networkidle");
  await expect.poll(() => page.locator("html").getAttribute("data-theme"), { timeout: 20_000 }).toBe(scheme);
  expect(errors, "the chat page threw").toEqual([]);
}
