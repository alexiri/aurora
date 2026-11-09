import { act, cleanup, renderHook, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { useTranscript } from "@/lib/chat/use-transcript";

// FIX-021 (FIX-019 finding): the C-501 SSE event *named* `error` also fires `EventSource.onerror` (readyState stays
// OPEN, `onopen` never re-fires). The pill must stay Live for it; only a real disconnect shows "Reconnecting…".

class FakeES {
  static last: FakeES;
  static CONNECTING = 0;
  static OPEN = 1;
  static CLOSED = 2;
  readyState = 1;
  listeners: Record<string, ((e: MessageEvent) => void)[]> = {};
  onopen: (() => void) | null = null;
  onerror: ((e: Event) => void) | null = null;
  constructor(public url: string) {
    FakeES.last = this;
  }
  addEventListener(t: string, fn: (e: MessageEvent) => void) {
    (this.listeners[t] ??= []).push(fn);
  }
  /** What a browser does for a named event: the listener of that type, and `onerror` too when the name is `error`. */
  named(type: string, data: unknown) {
    const ev = { type, data: JSON.stringify(data) } as MessageEvent;
    this.listeners[type]?.forEach((f) => f(ev));
    if (type === "error") this.onerror?.(ev);
  }
  /** A connection failure: a plain `Event`, no `data`. */
  disconnect(state = 0) {
    this.readyState = state;
    this.onerror?.({ type: "error" } as Event);
  }
  close() {}
}

beforeEach(() => {
  vi.stubGlobal("EventSource", FakeES);
  vi.stubGlobal(
    "fetch",
    vi.fn(async () => new Response(JSON.stringify({ items: [], last_seq: 0 }), { status: 200, headers: { "content-type": "application/json" } })),
  );
});
afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

describe("useTranscript live pill", () => {
  it("stays Live when the named `error` event arrives, and shows the error", async () => {
    const { result } = renderHook(() => useTranscript("c1"));
    await waitFor(() => expect(result.current.loading).toBe(false));
    act(() => FakeES.last.onopen?.());
    expect(result.current.live).toBe(true);
    act(() =>
      FakeES.last.named("error", { seq: 1, type: "error", ts: "2026-11-09T10:00:00Z", payload: { turn_id: "t1", code: "no_default_model", message: "m" } }),
    );
    expect(result.current.live).toBe(true);
    expect(result.current.state.events.map((e) => e.type)).toEqual(["error"]);
  });

  it("shows Reconnecting on a real disconnect and Live again on reopen", async () => {
    const { result } = renderHook(() => useTranscript("c1"));
    await waitFor(() => expect(result.current.loading).toBe(false));
    act(() => FakeES.last.onopen?.());
    expect(result.current.live).toBe(true);
    act(() => FakeES.last.disconnect(0));
    expect(result.current.live).toBe(false);
    act(() => FakeES.last.onopen?.());
    expect(result.current.live).toBe(true);
  });
});
