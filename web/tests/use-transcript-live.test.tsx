import { act, cleanup, renderHook, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { setRateNotice, type RateNotice } from "@/lib/api/rate-limit";
import { useTranscript } from "@/lib/chat/use-transcript";

// The module's current notice (no hook needed): subscribe-free read through a probe component is overkill.
let noticeValue: RateNotice = null;
const getNotice = () => noticeValue;
vi.mock("@/lib/api/rate-limit", async (orig) => {
  const real = await orig<typeof import("@/lib/api/rate-limit")>();
  return {
    ...real,
    setRateNotice: (n: RateNotice) => {
      noticeValue = n;
      real.setRateNotice(n);
    },
  };
});

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

// FIX-099: `EventSource` hides why it gave up. Only a 429 `too_many_streams` is the "too many open
// tabs" notice; a 5xx reconnects; a 401 sends the person to sign in.
describe("useTranscript refused stream", () => {
  const history = () => new Response(JSON.stringify({ items: [], last_seq: 0 }), { status: 200, headers: { "content-type": "application/json" } });
  const streamAnswer = (status: number, body: unknown = {}) =>
    vi.fn(async (input: RequestInfo | URL) =>
      String(input).includes("/stream") ? new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } }) : history(),
    );

  it("a 429 too_many_streams raises the streams notice and does not reconnect", async () => {
    vi.stubGlobal("fetch", streamAnswer(429, { error: { code: "too_many_streams" } }));
    const { result } = renderHook(() => useTranscript("c1"));
    await waitFor(() => expect(result.current.loading).toBe(false));
    const before = FakeES.last;
    act(() => before.disconnect(2));
    await waitFor(() => expect(getNotice()).toEqual({ kind: "streams" }));
    expect(FakeES.last).toBe(before);
    setRateNotice(null);
  });

  it("a 503 is not the streams notice: it reconnects with a fresh stream", async () => {
    setRateNotice(null);
    vi.stubGlobal("fetch", streamAnswer(503));
    const { result } = renderHook(() => useTranscript("c1"));
    await waitFor(() => expect(result.current.loading).toBe(false));
    const before = FakeES.last;
    act(() => before.disconnect(2));
    await waitFor(() => expect(FakeES.last).not.toBe(before), { timeout: 4000 });
    expect(getNotice()).toBeNull();
    expect(result.current.live).toBe(false);
  });

  it("a 401 sends the person to sign in", async () => {
    setRateNotice(null);
    const assign = vi.fn();
    vi.stubGlobal("location", { ...window.location, assign, pathname: "/o/acme/w/main/chat", search: "" });
    vi.stubGlobal("fetch", streamAnswer(401, { error: { code: "unauthorized" } }));
    const { result } = renderHook(() => useTranscript("c1"));
    await waitFor(() => expect(result.current.loading).toBe(false));
    act(() => FakeES.last.disconnect(2));
    await waitFor(() => expect(assign).toHaveBeenCalledWith("/auth/login?next=%2Fo%2Facme%2Fw%2Fmain%2Fchat"));
    expect(getNotice()).toBeNull();
  });
});
