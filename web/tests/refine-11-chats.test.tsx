import { act, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { ChatPane, ChatTab } from "@/components/agents/chat-tab";
import { listTime } from "@/lib/chat/layout";
import { resetStopped } from "@/lib/chat/stopped";
import { json } from "./support/api";
import { wrap } from "./support/render";

// P5-REFINE-11 items that waited for P5-REFINE-7 (D-409, C-1073..C-1075): several chats per person (New chat, the list,
// Rename, auto-titles), run threads folded under their automation, and the Stop mark.

const st = vi.hoisted(() => ({ events: [] as unknown[] }));
vi.mock("@/lib/chat/use-transcript", () => ({
  useTranscript: () => ({
    state: { events: st.events, deltas: {}, activity: {}, purged: {}, lastSeq: st.events.length },
    loading: false,
    error: null,
    live: true,
  }),
}));

const conv = (id: string, kind: string, over: Record<string, unknown> = {}) => ({
  id, agent_id: "a1", kind, external_ref: id, title: null, title_source: "auto", direct_user_id: null, direct_user: null, origin_conversation_id: null,
  last_seq: 0, app_turn_streak: 0, app_turn_capped: false, active_turn: null, last_activity_at: null, archived_at: null, automation: null, run_at: null,
  created_at: "2026-01-01T00:00:00Z", ...over,
});
interface Call { method: string; path: string; search: string; body: unknown }
function mock(routes: Record<string, (c: Call) => Response> = {}) {
  const calls: Call[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: Request | string | URL, init?: RequestInit) => {
      const req = input instanceof Request ? input : new Request(new URL(String(input), "http://localhost"), init);
      const url = new URL(req.url, "http://localhost");
      const text = req.method === "GET" || req.method === "DELETE" ? "" : await req.clone().text();
      const call = { method: req.method, path: url.pathname, search: url.search, body: text ? JSON.parse(text) : undefined };
      calls.push(call);
      const h = routes[`${req.method} ${url.pathname}`];
      return h ? h(call) : json(404, { error: { code: "not_found", message: "nf" } });
    }),
  );
  return calls;
}

const NOW_ISO = new Date().toISOString();
/** 09:00 local on the given day of November, this year (the zone the list shows). */
const AT = (day: number) => new Date(new Date().getFullYear(), 10, day, 9, 0).toISOString();
// An older chat: far enough back that the list shows its date, not a weekday.
const OLD = new Date(Date.now() - 30 * 86_400_000);
const chats = [
  conv("d2", "direct", { title: "Quarterly planning summary", last_activity_at: NOW_ISO }),
  conv("d1", "direct", { title: "Hello Daneel", last_activity_at: OLD.toISOString() }),
];
const e = (seq: number, type: string, payload: Record<string, unknown>) => ({ seq, type, ts: "2026-11-08T12:00:00Z", payload });

beforeEach(() => {
  st.events = [];
  window.history.replaceState(null, "", "/");
});
afterEach(() => {
  resetStopped();
  vi.unstubAllGlobals();
});

const tab = () => wrap(<ChatTab agentId="a1" wsId="w" base="/o/x/w/y" agent={{ id: "a1", name: "Daneel", handle: "daneel" }} />);

describe("several chats (chat-3, D-409)", () => {
  const routes = (extra: Record<string, (c: Call) => Response> = {}) => ({
    "POST /v1/agents/a1/conversations/direct": () => json(200, chats[0]),
    "GET /v1/agents/a1/conversations": () => json(200, { items: chats, next_cursor: null }),
    "GET /v1/agents/a1/conversation-folders": () => json(200, { items: [], next_cursor: null }),
    ...extra,
  });

  it("lists the person's chats newest first under a Conversations heading with a New chat button; the Active list leaves run threads out", async () => {
    const calls = mock(routes());
    tab();
    const nav = await screen.findByRole("navigation", { name: "Conversations" });
    expect(within(nav).getByRole("heading", { name: "Conversations" })).toBeTruthy();
    expect(within(nav).getByRole("button", { name: "New chat" })).toBeTruthy();
    await within(nav).findByText("Hello Daneel");
    const titles = within(nav).getAllByRole("button").map((b) => b.textContent ?? "").filter((t) => /Quarterly|Hello/.test(t));
    expect(titles[0]).toMatch(/Quarterly planning summary/);
    expect(titles[1]).toMatch(/Hello Daneel/);
    // Today's row names the pair and the time; an older one just the date.
    expect(within(nav).getByText(/^You and Daneel · \d\d:\d\d$/)).toBeTruthy();
    expect(within(nav).getByText(listTime(OLD.toISOString(), new Date()))).toBeTruthy();
    expect(calls.some((c) => c.path === "/v1/agents/a1/conversations" && c.search.includes("automation=none") && c.search.includes("archived=false"))).toBe(true);
  });

  it("New chat opens a fresh conversation at once (the previous ones stay), with the composer focused", async () => {
    const calls = mock(routes({ "POST /v1/agents/a1/conversations/direct/new": () => json(201, conv("d3", "direct")) }));
    tab();
    const nav = await screen.findByRole("navigation", { name: "Conversations" });
    await within(nav).findByText("Hello Daneel");
    fireEvent.click(within(nav).getByRole("button", { name: "New chat" }));
    await waitFor(() => expect(calls.some((c) => c.method === "POST" && c.path === "/v1/agents/a1/conversations/direct/new")).toBe(true));
    await waitFor(() => expect(window.location.search).toBe("?c=d3"));
    await screen.findByRole("heading", { name: "Say hello to Daneel" });
    await waitFor(() => expect(document.activeElement).toBe(screen.getByLabelText("Message")));
  });

  it("says why New chat failed past the limit", async () => {
    mock(routes({ "POST /v1/agents/a1/conversations/direct/new": () => json(422, { error: { code: "conversation_limit", message: "limit" } }) }));
    tab();
    const nav = await screen.findByRole("navigation", { name: "Conversations" });
    fireEvent.click(within(nav).getByRole("button", { name: "New chat" }));
    expect(await within(nav).findByRole("alert")).toHaveTextContent(/limit of chats/);
  });

  it("Rename edits the title in place: Enter saves (PATCH), shows Saved; Esc cancels without a call", async () => {
    const calls = mock(
      routes({ "PATCH /v1/conversations/d1": () => json(200, { ...chats[1], title: "Greeting", title_source: "person" }) }),
    );
    tab();
    const nav = await screen.findByRole("navigation", { name: "Conversations" });
    await within(nav).findByText("Hello Daneel");
    const open = () => fireEvent.keyDown(within(nav).getByRole("button", { name: "Actions for Hello Daneel" }), { key: "Enter" });
    open();
    fireEvent.click(screen.getByRole("menuitem", { name: "Rename" }));
    const field = within(nav).getByLabelText("Conversation title");
    expect(field).toHaveValue("Hello Daneel");
    fireEvent.keyDown(field, { key: "Escape" });
    expect(within(nav).queryByLabelText("Conversation title")).toBeNull();
    expect(calls.some((c) => c.method === "PATCH")).toBe(false);

    open();
    fireEvent.click(screen.getByRole("menuitem", { name: "Rename" }));
    const again = within(nav).getByLabelText("Conversation title");
    fireEvent.change(again, { target: { value: "Greeting" } });
    fireEvent.keyDown(again, { key: "Enter" });
    await waitFor(() => expect(calls.find((c) => c.method === "PATCH")?.body).toEqual({ title: "Greeting" }));
    expect(await within(nav).findByText("Saved")).toBeTruthy();
  });

  it("the row menu offers Rename, Archive and Delete this conversation…", async () => {
    mock(routes());
    tab();
    const nav = await screen.findByRole("navigation", { name: "Conversations" });
    await within(nav).findByText("Hello Daneel");
    fireEvent.keyDown(within(nav).getByRole("button", { name: "Actions for Hello Daneel" }), { key: "Enter" });
    expect(screen.getAllByRole("menuitem").map((m) => m.textContent)).toEqual(["Rename", "Archive", "Delete this conversation…"]);
  });

  it("another member's direct chat has no menu; a run thread's menu has no Rename", async () => {
    const run = conv("r1", "schedule", {
      title: "Morning summary", title_source: "automation", last_activity_at: NOW_ISO,
      automation: { kind: "schedule", id: "s1", name: "Morning summary", deleted: false }, run_at: AT(19),
    });
    mock(
      routes({
        "GET /v1/agents/a1/conversations": () =>
          json(200, { items: [...chats, conv("ann", "direct", { direct_user: { id: "u2", display_name: "Ann" }, last_activity_at: NOW_ISO }), run], next_cursor: null }),
      }),
    );
    tab();
    const nav = await screen.findByRole("navigation", { name: "Conversations" });
    await within(nav).findByText("Ann");
    expect(within(nav).queryByRole("button", { name: "Actions for Ann" })).toBeNull();
    fireEvent.keyDown(within(nav).getByRole("button", { name: /^Actions for Morning summary/ }), { key: "Enter" });
    expect(screen.queryByRole("menuitem", { name: "Rename" })).toBeNull();
  });
});

describe("run threads folded under their automation (D-409, C-1074)", () => {
  it("shows 'Morning summary · 3 runs' collapsed; opening it loads that automation's threads, each titled with its date", async () => {
    const run = (id: string, at: string) =>
      conv(id, "schedule", { title: "Morning summary", title_source: "automation", automation: { kind: "schedule", id: "s1", name: "Morning summary", deleted: false }, run_at: at, last_activity_at: at });
    const calls = mock({
      "POST /v1/agents/a1/conversations/direct": () => json(200, chats[0]),
      "GET /v1/agents/a1/conversations": (c) =>
        json(200, { items: c.search.includes("automation=only") ? [run("r3", AT(19)), run("r2", AT(18)), run("r1", AT(17))] : chats, next_cursor: null }),
      "GET /v1/agents/a1/conversation-folders": () =>
        json(200, { items: [{ kind: "schedule", id: "s1", name: "Morning summary", deleted: false, thread_count: 3, last_activity_at: AT(19), last_result: "answered" }], next_cursor: null }),
    });
    tab();
    const nav = await screen.findByRole("navigation", { name: "Conversations" });
    const folder = await within(nav).findByRole("button", { name: /Morning summary · 3 runs/ });
    expect(folder).toHaveAttribute("aria-expanded", "false");
    expect(within(nav).queryByText(/Morning summary · 19 Nov 09:00/)).toBeNull();
    expect(calls.some((c) => c.search.includes("automation=only"))).toBe(false);
    fireEvent.click(folder);
    expect(await within(nav).findByText("Morning summary · 19 Nov 09:00")).toBeTruthy();
    expect(within(nav).getByText("Morning summary · 18 Nov 09:00")).toBeTruthy();
    expect(within(nav).getByText("Morning summary · 17 Nov 09:00")).toBeTruthy();
    expect(calls.some((c) => c.search.includes("automation=only") && c.search.includes("automation_id=s1"))).toBe(true);
    expect(folder).toHaveAttribute("aria-expanded", "true");
  });

  it("marks a folder whose automation was deleted", async () => {
    mock({
      "POST /v1/agents/a1/conversations/direct": () => json(200, chats[0]),
      "GET /v1/agents/a1/conversations": () => json(200, { items: chats, next_cursor: null }),
      "GET /v1/agents/a1/conversation-folders": () =>
        json(200, { items: [{ kind: "webhook", id: "w1", name: "Deploys", deleted: true, thread_count: 1, last_activity_at: "2026-11-10T09:00:00Z", last_result: null }], next_cursor: null }),
    });
    tab();
    const nav = await screen.findByRole("navigation", { name: "Conversations" });
    expect(await within(nav).findByText("Webhook deleted")).toBeTruthy();
  });
});

describe("Stop keeps what was written (chat-4, C-1075)", () => {
  const pane = () => wrap(<ChatPane agentId="a1" wsId="w" base="/o/x/w/y" convId="c1" direct={null} agent={{ id: "a1", name: "Daneel", handle: "daneel" }} />);

  it("shows 'You stopped this answer' under the partial answer", () => {
    st.events = [
      e(1, "turn.queued", { turn_id: "t1", speaker_label: "Bob", text: "Write a long story" }),
      e(2, "text", { turn_id: "t1", text: "Chapter 1. Mara climbed the steps…", partial: true }),
      e(3, "turn.finished", { turn_id: "t1", status: "aborted" }),
      e(4, "notice", { code: "turn_stopped", text: "You stopped this answer.", turn_id: "t1" }),
    ];
    mock();
    pane();
    expect(screen.getByText(/Chapter 1\. Mara climbed the steps/)).toBeTruthy();
    expect(within(screen.getByTestId("turn-stopped")).getByText("You stopped this answer")).toBeTruthy();
  });

  it("Stop is disabled and reads 'Stopping…' once pressed, and stays so until the turn ends", async () => {
    st.events = [e(1, "turn.queued", { turn_id: "t1", speaker_label: "Bob", text: "Write a long story" })];
    const calls = mock({ "POST /v1/turns/t1/abort": () => json(200, { id: "t1", abort_requested: true }) });
    pane();
    fireEvent.click(screen.getByRole("button", { name: "Stop" }));
    await waitFor(() => expect(calls.some((c) => c.path === "/v1/turns/t1/abort")).toBe(true));
    const stopping = await screen.findByRole("button", { name: "Stopping…" });
    expect(stopping).toBeDisabled();
    await act(async () => {});
    expect(screen.getByRole("button", { name: "Stopping…" })).toBeDisabled();
  });
});
