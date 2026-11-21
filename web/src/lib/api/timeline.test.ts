// (P5-REFINE-9, C-1084) The timeline client: an upsert merges by id (in place, or a new row on top),
// rows that arrive while the list is scrolled wait as `pendingNew`, and `resync` refetches.
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, renderHook } from "@testing-library/react";
import { createElement, type ReactNode } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";

import {
  matchesFilters,
  mergeUpsert,
  parseRowEvent,
  timelineKeys,
  useTimelineStream,
  type TimelineData,
  type TimelineItem,
} from "./timeline";

function item(id: string, over: Partial<TimelineItem> = {}): TimelineItem {
  return {
    id,
    at: "2026-11-20T10:00:00Z",
    ended_at: null,
    state: "done",
    kind: "called",
    outcome: "allowed",
    agent: { id: "a1", name: "Dani" },
    person: null,
    app: null,
    tool: null,
    host: "api.github.com",
    rule: null,
    model: null,
    schedule: null,
    webhook: null,
    peer: null,
    count: 1,
    command: null,
    files: null,
    duration_ms: null,
    link: null,
    ...over,
  };
}

const data = (...pages: TimelineItem[][]): TimelineData => ({
  pages: pages.map((items, i) => ({ items, next_cursor: i < pages.length - 1 ? `c${i}` : null })),
  pageParams: pages.map((_, i) => (i === 0 ? undefined : `c${i - 1}`)),
});

describe("mergeUpsert", () => {
  it("replaces a row with the same id where it is", () => {
    const d = data([item("a"), item("run:1", { state: "running", outcome: null })], [item("b")]);
    const next = mergeUpsert(d, item("run:1", { state: "done", outcome: "answered" }))!;
    expect(next.pages[0].items.map((i) => i.id)).toEqual(["a", "run:1"]);
    expect(next.pages[0].items[1]).toMatchObject({ state: "done", outcome: "answered" });
    expect(next.pages[1]).toBe(d.pages[1]);
  });

  it("puts a new id at the top of the first page", () => {
    const d = data([item("a")], [item("b")]);
    const next = mergeUpsert(d, item("new"))!;
    expect(next.pages[0].items.map((i) => i.id)).toEqual(["new", "a"]);
    expect(next.pages[1].items.map((i) => i.id)).toEqual(["b"]);
  });

  it("leaves an empty cache alone", () => {
    expect(mergeUpsert(undefined, item("x"))).toBeUndefined();
  });
});

describe("parseRowEvent and filters", () => {
  it("accepts only upserts with an item", () => {
    expect(parseRowEvent(JSON.stringify({ op: "upsert", item: item("a") }))?.item.id).toBe("a");
    expect(parseRowEvent("{")).toBeNull();
    expect(parseRowEvent(JSON.stringify({ op: "delete", item: item("a") }))).toBeNull();
  });

  it("matches kinds and outcomes", () => {
    expect(matchesFilters(item("a"), { kinds: ["called"] })).toBe(true);
    expect(matchesFilters(item("a"), { kinds: ["computer"] })).toBe(false);
    expect(matchesFilters(item("a", { outcome: null }), { outcomes: ["allowed"] })).toBe(false);
  });
});

describe("useTimelineStream", () => {
  const listeners: Record<string, (e: unknown) => void> = {};
  let url = "";
  class FakeES {
    static OPEN = 1;
    static CLOSED = 2;
    readyState = 1;
    onopen: (() => void) | null = null;
    onerror: (() => void) | null = null;
    constructor(u: string) {
      url = u;
    }
    addEventListener(t: string, f: (e: unknown) => void) {
      listeners[t] = f;
    }
    close() {}
  }

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  function setup(atTop = true, technical = false) {
    vi.stubGlobal("EventSource", FakeES);
    const qc = new QueryClient();
    qc.setQueryData(timelineKeys.list("a1"), data([item("old")]));
    qc.setQueryData(timelineKeys.list("a1", { kinds: ["computer"] }), data([]));
    const wrapper = ({ children }: { children: ReactNode }) => createElement(QueryClientProvider, { client: qc }, children);
    const hook = renderHook((p: { atTop: boolean }) => useTimelineStream("a1", { atTop: p.atTop, technical }), {
      wrapper,
      initialProps: { atTop },
    });
    const send = (it: TimelineItem) => act(() => listeners.row?.({ data: JSON.stringify({ op: "upsert", item: it }) }));
    return { qc, hook, send };
  }

  it("merges upserts into every list of the agent that wants them", () => {
    const { qc, send } = setup();
    expect(url).toBe("/v1/agents/a1/timeline/stream");
    send(item("new"));
    expect(qc.getQueryData<TimelineData>(timelineKeys.list("a1"))!.pages[0].items.map((i) => i.id)).toEqual(["new", "old"]);
    // The computer-only list does not take a `called` row.
    expect(qc.getQueryData<TimelineData>(timelineKeys.list("a1", { kinds: ["computer"] }))!.pages[0].items).toEqual([]);
    send(item("new", { state: "failed", outcome: "failed" }));
    const rows = qc.getQueryData<TimelineData>(timelineKeys.list("a1"))!.pages[0].items;
    expect(rows.map((i) => i.id)).toEqual(["new", "old"]);
    expect(rows[0].outcome).toBe("failed");
  });

  it("counts rows that arrive while the list is scrolled, and shows them on request", () => {
    const { qc, hook, send } = setup(false);
    send(item("n1"));
    send(item("n2"));
    send(item("n1", { outcome: "blocked_by_rule" }));
    expect(hook.result.current.pendingNew).toBe(2);
    expect(qc.getQueryData<TimelineData>(timelineKeys.list("a1"))!.pages[0].items.map((i) => i.id)).toEqual(["old"]);
    // A row already shown still updates in place while scrolled.
    send(item("old", { outcome: "rate_limited" }));
    expect(qc.getQueryData<TimelineData>(timelineKeys.list("a1"))!.pages[0].items[0].outcome).toBe("rate_limited");
    act(() => hook.result.current.showNew());
    expect(hook.result.current.pendingNew).toBe(0);
    const rows = qc.getQueryData<TimelineData>(timelineKeys.list("a1"))!.pages[0].items;
    expect(rows.map((i) => i.id)).toEqual(["n2", "n1", "old"]);
    expect(rows[1].outcome).toBe("blocked_by_rule");
  });

  it("shows the waiting rows when the list scrolls back to the top", () => {
    const { qc, hook, send } = setup(false);
    send(item("n1"));
    hook.rerender({ atTop: true });
    expect(hook.result.current.pendingNew).toBe(0);
    expect(qc.getQueryData<TimelineData>(timelineKeys.list("a1"))!.pages[0].items.map((i) => i.id)).toEqual(["n1", "old"]);
  });

  it("refetches on resync and asks for request events only with technical", () => {
    const { qc } = setup(true, true);
    expect(url).toBe("/v1/agents/a1/timeline/stream?technical=true");
    const spy = vi.spyOn(qc, "invalidateQueries");
    act(() => listeners.resync?.({}));
    expect(spy).toHaveBeenCalledWith({ queryKey: ["timeline", "a1"] });
  });
});
