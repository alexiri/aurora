import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, renderHook, waitFor } from "@testing-library/react";
import type { ReactNode } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { useAgentStatus } from "@/components/agents/status-rail/status-data";
import type { TimelineItem } from "@/lib/api/timeline";

import { json } from "./support/api";

// (P5-REFINE-11 item 3, D-415) "What <Agent> did" is the agent's last five timeline rows, live over the timeline stream:
// a new row goes on top, a running row turns into its outcome in place, and only five are kept.

class FakeES {
  static last: FakeES;
  listeners: Record<string, ((e: MessageEvent) => void)[]> = {};
  onopen: (() => void) | null = null;
  onerror: (() => void) | null = null;
  constructor(public url: string) {
    FakeES.last = this;
  }
  addEventListener(t: string, fn: (e: MessageEvent) => void) {
    (this.listeners[t] ??= []).push(fn);
  }
  emit(t: string, data: unknown) {
    this.listeners[t]?.forEach((f) => f({ data: JSON.stringify(data) } as MessageEvent));
  }
  close() {}
}

const row = (id: string, over: Partial<TimelineItem> = {}): TimelineItem => ({
  id,
  at: "2026-11-20T10:00:00Z",
  state: "done",
  kind: "answered",
  outcome: "answered",
  agent: { id: "a1", name: "Scout" },
  ...over,
});

let paths: string[];
let queries: string[];

beforeEach(() => {
  paths = [];
  queries = [];
  FakeES.last = undefined as never;
  vi.stubGlobal("EventSource", FakeES);
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: Request) => {
      const url = new URL(input.url);
      paths.push(url.pathname);
      if (url.pathname === "/v1/agents/a1/timeline") {
        queries.push(url.search);
        return json(200, {
          items: [row("run:1", { kind: "ran_schedule", state: "running", outcome: null, schedule: { id: "s", name: "Morning summary" } }), row("turn:2"), row("turn:3")],
          next_cursor: null,
        });
      }
      return json(404, { error: { code: "not_found", message: "nf" } });
    }),
  );
});

function status() {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const Wrapper = ({ children }: { children: ReactNode }) => <QueryClientProvider client={qc}>{children}</QueryClientProvider>;
  return renderHook(() => useAgentStatus({ wsId: "w1", agentId: "a1", canApprove: false, conversations: [] }), { wrapper: Wrapper });
}

describe("What <Agent> did, live", () => {
  it("asks the timeline for five rows and opens one stream for the agent", async () => {
    const { result } = status();
    await waitFor(() => expect(result.current.did.map((r) => r.id)).toEqual(["run:1", "turn:2", "turn:3"]));
    expect(queries[0]).toContain("limit=5");
    expect(FakeES.last.url).toBe("/v1/agents/a1/timeline/stream");
    // The old request-log read is gone: the rail no longer asks the workspace activity route.
    expect(paths.filter((p) => p.endsWith("/activity"))).toEqual([]);
  });

  it("puts a new row on top, turns a running row into its outcome in place, and keeps five", async () => {
    const { result } = status();
    await waitFor(() => expect(result.current.did).toHaveLength(3));
    act(() => FakeES.last.onopen?.());
    act(() => FakeES.last.emit("row", { op: "upsert", item: row("call:9", { kind: "called", state: "blocked", outcome: "blocked_by_rule" }) }));
    await waitFor(() => expect(result.current.did.map((r) => r.id)).toEqual(["call:9", "run:1", "turn:2", "turn:3"]));
    expect(result.current.problem).toBe(true);

    act(() => FakeES.last.emit("row", { op: "upsert", item: row("run:1", { kind: "ran_schedule", state: "done", outcome: "answered", schedule: { id: "s", name: "Morning summary" } }) }));
    await waitFor(() => expect(result.current.did[1]).toMatchObject({ id: "run:1", state: "done", outcome: "answered" }));
    expect(result.current.did.map((r) => r.id)).toEqual(["call:9", "run:1", "turn:2", "turn:3"]);

    act(() => FakeES.last.emit("row", { op: "upsert", item: row("turn:10") }));
    act(() => FakeES.last.emit("row", { op: "upsert", item: row("turn:11") }));
    await waitFor(() => expect(result.current.did.map((r) => r.id)).toEqual(["turn:11", "turn:10", "call:9", "run:1", "turn:2"]));
  });
});
