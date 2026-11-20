import { act, cleanup, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { ActivityView } from "@/components/approvals/activity-view";
import { ACTIVITY_KIND, ACTIVITY_KIND_FILTER, timelineOutcomeGroup } from "@/lib/labels";
import type { TimelineItem } from "@/lib/api/timeline";
import { MAX_LIVE_AGENTS } from "@/lib/api/workspace-timeline";
import { wrap } from "./support/render";
import { json } from "./support/api";

// P5-REFINE-16 workspace-2 (parts A and B, D-415): the sentences a person reads from the agents' timelines, the filters in
// words, the recorded fields under Details, and the live feed: a new row on top, a running row turning into its outcome in
// place, "n new · Show" while the page is scrolled, the first agents streamed and the rest polled.

class FakeES {
  static all: FakeES[] = [];
  listeners: Record<string, ((e: MessageEvent) => void)[]> = {};
  onopen: (() => void) | null = null;
  onerror: (() => void) | null = null;
  constructor(public url: string) {
    FakeES.all.push(this);
  }
  addEventListener(t: string, fn: (e: MessageEvent) => void) {
    (this.listeners[t] ??= []).push(fn);
  }
  emit(t: string, data: unknown) {
    this.listeners[t]?.forEach((f) => f({ data: JSON.stringify(data) } as MessageEvent));
  }
  close() {}
}

const DANI = { id: "a1", name: "Dani" };
const row = (id: string, over: Partial<TimelineItem>): TimelineItem => ({
  id,
  at: new Date(Date.now() - 60_000).toISOString(),
  state: "done",
  kind: "answered",
  outcome: "answered",
  agent: DANI,
  ...over,
});
const answered = row("turn:1", { model: "Workspace default", count: 1, duration_ms: 2300 });
const called = row("call:1", {
  kind: "called",
  state: "blocked",
  outcome: "blocked_by_rule",
  app: { id: "github", name: "GitHub" },
  rule: { logical_id: "r1", name: "Block evil.example.org" },
  count: 1,
  at: new Date(Date.now() - 120_000).toISOString(),
});
const asked = row("approval:1", { kind: "asked_before", state: "waiting", outcome: null, host: "docs.example.org", at: new Date(Date.now() - 180_000).toISOString() });

let timelineQueries: string[];
let activityCalls: number;
let agentsList: { id: string; name: string; handle: string }[];
let feeds: Record<string, TimelineItem[]>;

beforeEach(() => {
  FakeES.all = [];
  timelineQueries = [];
  activityCalls = 0;
  agentsList = [{ id: "a1", name: "Dani", handle: "dani" }];
  feeds = { a1: [answered, called, asked] };
  vi.stubGlobal("EventSource", FakeES);
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: Request) => {
      const url = new URL(input.url, "http://localhost");
      const m = /^\/v1\/agents\/([^/]+)\/timeline$/.exec(url.pathname);
      if (m) {
        timelineQueries.push(`${m[1]}${url.search}`);
        return json(200, { items: feeds[m[1]!] ?? [], next_cursor: null });
      }
      if (url.pathname === "/v1/workspaces/w1/activity") {
        activityCalls += 1;
        return json(200, { items: [], next_cursor: null });
      }
      if (url.pathname === "/v1/workspaces/w1/agents") return json(200, { items: agentsList, next_cursor: null });
      return json(404, { error: { code: "not_found", message: "nf" } });
    }),
  );
});

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
  Object.defineProperty(window, "scrollY", { value: 0, configurable: true });
});

const show = () => wrap(<ActivityView wsId="w1" base="/o/x/w/y" />, { moreControls: false });
const titles = (list: HTMLElement) => within(list).getAllByRole("listitem").map((li) => li.querySelector("p")?.textContent);

describe("the filters and outcome groups", () => {
  it("offers Everything · Answers · Calls to apps · Approvals · Automations and maps each to timeline kinds", () => {
    expect(Object.values(ACTIVITY_KIND).map((k) => k.label)).toEqual(["Everything", "Answers", "Calls to apps", "Approvals", "Automations"]);
    expect(ACTIVITY_KIND_FILTER.automation).toEqual(["ran_schedule", "received_webhook"]);
    expect(ACTIVITY_KIND_FILTER.app).toEqual(["called"]);
  });
  it("groups a row as Waited for you, Blocked, Failed or Answered", () => {
    const g = (kind: TimelineItem["kind"], state: TimelineItem["state"], outcome: string | null) => timelineOutcomeGroup({ kind, state, outcome });
    expect(g("asked_before", "done", "allowed_once")).toBe("waited");
    expect(g("answered", "waiting", null)).toBe("waited");
    expect(g("called", "blocked", "blocked_by_rule")).toBe("blocked");
    expect(g("called", "done", "rate_limited")).toBe("blocked");
    expect(g("received_webhook", "blocked", "refused_signature")).toBe("blocked");
    expect(g("ran_schedule", "failed", "failed")).toBe("failed");
    expect(g("answered", "done", "answered")).toBe("answered");
  });
});

describe("ActivityView sentences", () => {
  it("shows sentence rows, filters in words, and the recorded fields (never a request path) under Details", async () => {
    show();
    const list = await screen.findByTestId("activity-sentences");
    expect(titles(list)).toEqual(["Dani answered you", "Dani called GitHub", "Dani asked before changing docs.example.org"]);
    expect(within(list).getByText("Used Workspace default · 1 call")).toBeInTheDocument();
    expect(within(list).getByText("Blocked by your rule ‘Block evil.example.org’")).toBeInTheDocument();
    expect(within(list).getByRole("link", { name: "Waiting for you in Approvals" }).getAttribute("href")).toBe("/o/x/w/y/approvals");
    expect(screen.getByRole("option", { name: "Any outcome" })).toBeInTheDocument();
    expect(screen.getByRole("option", { name: "Automations" })).toBeInTheDocument();
    expect(screen.getByRole("option", { name: "Waited for you" })).toBeInTheDocument();
    expect(screen.queryByText(/blocked_by_rule|default_denied/)).toBeNull();
    fireEvent.click(within(list).getAllByRole("button", { name: "Details" })[0]!);
    const raw = screen.getByTestId("activity-raw");
    expect(raw).toHaveTextContent("Details · what was recorded");
    expect(raw).toHaveTextContent("kind answered · state done · outcome answered · model Workspace default · count 1 · took 2.3 s");
    expect(raw).not.toHaveTextContent("/v1/messages");
  });

  it("the outcome filter narrows the list", async () => {
    show();
    const list = await screen.findByTestId("activity-sentences");
    fireEvent.change(screen.getByLabelText("Outcome"), { target: { value: "blocked" } });
    expect(titles(list)).toEqual(["Dani called GitHub"]);
    fireEvent.change(screen.getByLabelText("Outcome"), { target: { value: "waited" } });
    expect(titles(list)).toEqual(["Dani asked before changing docs.example.org"]);
  });

  it("the kind filter asks the API for those kinds (Automations: runs and deliveries)", async () => {
    show();
    await screen.findByTestId("activity-sentences");
    fireEvent.change(screen.getByLabelText("Kind"), { target: { value: "automation" } });
    await waitFor(() => expect(timelineQueries.some((q) => q.includes("kind=ran_schedule%2Creceived_webhook") || q.includes("kind=ran_schedule,received_webhook"))).toBe(true));
  });
});

describe("the live timeline", () => {
  const open = async () => {
    show();
    const list = await screen.findByTestId("activity-sentences");
    await waitFor(() => expect(FakeES.all.length).toBe(1));
    act(() => FakeES.all[0]!.onopen?.());
    return list;
  };

  it("puts a new row on top and turns a waiting row into its outcome in place", async () => {
    const list = await open();
    act(() => FakeES.all[0]!.emit("row", { op: "upsert", item: row("run:9", { kind: "ran_schedule", state: "running", outcome: null, schedule: { id: "s", name: "Morning summary" }, at: new Date().toISOString() }) }));
    await waitFor(() => expect(titles(list)[0]).toBe("Running ‘Morning summary’…"));
    act(() => FakeES.all[0]!.emit("row", { op: "upsert", item: row("run:9", { kind: "ran_schedule", state: "done", outcome: "answered", schedule: { id: "s", name: "Morning summary" }, at: new Date().toISOString() }) }));
    await waitFor(() => expect(titles(list)[0]).toBe("Ran ‘Morning summary’"));
    expect(titles(list)).toHaveLength(4);
    // The approval that was waiting is decided: the same id, in place.
    act(() => FakeES.all[0]!.emit("row", { op: "upsert", item: { ...asked, state: "done", outcome: "allowed_once" } }));
    await waitFor(() => expect(within(list).queryByRole("link", { name: "Waiting for you in Approvals" })).toBeNull());
    expect(titles(list)).toHaveLength(4);
  });

  it("while the page is scrolled a new row waits behind '1 new · Show'", async () => {
    const list = await open();
    Object.defineProperty(window, "scrollY", { value: 600, configurable: true });
    act(() => {
      fireEvent.scroll(window);
    });
    act(() => FakeES.all[0]!.emit("row", { op: "upsert", item: row("turn:7", { at: new Date().toISOString(), model: "m" }) }));
    const pill = await screen.findByRole("button", { name: "1 new · Show" });
    expect(within(list).getAllByRole("listitem")).toHaveLength(3);
    const scrollTo = vi.fn();
    vi.stubGlobal("scrollTo", scrollTo);
    Object.defineProperty(window, "scrollY", { value: 0, configurable: true });
    fireEvent.click(pill);
    await waitFor(() => expect(within(list).getAllByRole("listitem")).toHaveLength(4));
    expect(screen.queryByRole("button", { name: /new · Show/ })).toBeNull();
  });

  it("streams the first agents and polls the rest (a person holds five streams in all)", async () => {
    agentsList = Array.from({ length: 7 }, (_, i) => ({ id: `g${i}`, name: `Agent ${i}`, handle: `agent-${i}` }));
    feeds = {};
    show();
    await waitFor(() => expect(timelineQueries.length).toBe(7));
    expect(FakeES.all.map((e) => e.url)).toEqual(Array.from({ length: MAX_LIVE_AGENTS }, (_, i) => `/v1/agents/g${i}/timeline/stream`));
    expect(await screen.findByText("No activity")).toBeInTheDocument();
  });

  it("opens the streams with technical=true at Expert and re-reads the Technical view on a request", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    try {
      wrap(<ActivityView wsId="w1" base="/o/x/w/y" />, { moreControls: true });
      await screen.findByTestId("activity-sentences");
      await waitFor(() => expect(FakeES.all.length).toBe(1));
      expect(FakeES.all[0]!.url).toBe("/v1/agents/a1/timeline/stream?technical=true");
      const before = activityCalls;
      act(() => FakeES.all[0]!.emit("request", { id: "q1", ts: new Date().toISOString(), agent_id: "a1", method: "GET", host: null, decision: "allow", status: 200 }));
      await act(async () => {
        await vi.advanceTimersByTimeAsync(1200);
      });
      await waitFor(() => expect(activityCalls).toBeGreaterThan(before));
    } finally {
      vi.useRealTimers();
    }
  });
});
