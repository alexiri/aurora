import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { ApprovalCard } from "@/components/agents/chat-tab";
import type { TranscriptEvent } from "@/lib/api/conversations";
import {
  activeTurnId,
  applyEvent,
  applyEvents,
  classifySend,
  emptyTranscript,
  errorCopy,
  loadHistory,
  pendingFollowups,
} from "@/lib/chat/transcript";

const ev = (seq: number | null, type: string, payload: Record<string, unknown>): TranscriptEvent => ({
  seq, type, ts: "2026-01-01T00:00:00Z", payload,
});

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

describe("transcript reducer", () => {
  it("dedupes by seq and orders", () => {
    const s = applyEvents(emptyTranscript, [ev(2, "turn.started", { turn_id: "t" }), ev(1, "turn.queued", { turn_id: "t" }), ev(2, "turn.started", { turn_id: "t" })]);
    expect(s.events.map((e) => e.seq)).toEqual([1, 2]);
    expect(s.lastSeq).toBe(2);
  });
  it("accumulates deltas and discards them when durable text arrives", () => {
    let s = applyEvent(emptyTranscript, ev(null, "text.delta", { turn_id: "t", message_id: "m", delta: "Hel" }));
    s = applyEvent(s, ev(null, "text.delta", { turn_id: "t", message_id: "m", delta: "lo" }));
    expect(s.deltas.m.text).toBe("Hello");
    s = applyEvent(s, ev(5, "text", { turn_id: "t", message_id: "m", text: "Hello!" }));
    expect(s.deltas).toEqual({});
  });
  it("clears activity and deltas on turn.finished", () => {
    let s = applyEvent(emptyTranscript, ev(null, "activity", { turn_id: "t", line: "working" }));
    s = applyEvent(s, ev(null, "text.delta", { turn_id: "t", message_id: "m", delta: "x" }));
    s = applyEvent(s, ev(3, "turn.finished", { turn_id: "t", status: "done" }));
    expect(s.activity).toEqual({});
    expect(s.deltas).toEqual({});
  });
  it("ignores unknown types without losing seq", () => {
    const s = applyEvent(emptyTranscript, ev(null, "mystery", {}));
    expect(s).toBe(emptyTranscript);
    expect(applyEvent(emptyTranscript, ev(4, "mystery", {})).lastSeq).toBe(4);
  });
  it("tracks the active turn", () => {
    let s = applyEvent(emptyTranscript, ev(1, "turn.queued", { turn_id: "t" }));
    expect(activeTurnId(s)).toBe("t");
    s = applyEvent(s, ev(2, "turn.finished", { turn_id: "t", status: "done" }));
    expect(activeTurnId(s)).toBeNull();
  });
});

describe("resume", () => {
  it("pages history until last_seq and returns the seq to stream from", async () => {
    const pages = [
      { items: [ev(1, "turn.queued", { turn_id: "t" }), ev(2, "turn.started", { turn_id: "t" })], last_seq: 3 },
      { items: [ev(3, "text", { turn_id: "t", message_id: "m", text: "hi" })], last_seq: 3 },
    ];
    const calls: number[] = [];
    const s = await loadHistory(async (after) => {
      calls.push(after);
      return pages.shift()!;
    });
    expect(calls).toEqual([0, 2]);
    expect(s.lastSeq).toBe(3);
  });
});

describe("composer", () => {
  it("classifies turn vs follow-up", () => {
    const turn = { id: "t1" } as never;
    const followup = { id: "f1", parent_turn_id: "t0", status: "joining", text: "x", created_at: "" } as never;
    expect(classifySend({ turn, followup: null })).toEqual({ kind: "turn", turnId: "t1" });
    expect(classifySend({ turn: null, followup }).kind).toBe("followup");
  });
  it("drops follow-ups once the parent turn finishes", () => {
    const f = { id: "f", parent_turn_id: "t", status: "joining", text: "x", created_at: "" } as const;
    expect(pendingFollowups([f], emptyTranscript)).toHaveLength(1);
    const s = applyEvent(emptyTranscript, ev(1, "message.sent_now", { turn_id: "t" }));
    expect(pendingFollowups([f], s)).toHaveLength(0);
  });
});

describe("error mapping", () => {
  it("maps known codes with link and falls back for unknown", () => {
    expect(errorCopy("no_model_key").link?.path).toBe("credentials");
    expect(errorCopy("weird").message).toMatch(/Something went wrong/);
    expect(errorCopy(null).link).toBeUndefined();
  });
});

describe("ApprovalCard", () => {
  it("posts the decision", async () => {
    const fn = vi.fn(async (r: Request) =>
      new Response(JSON.stringify(r.url.endsWith("/v1/workspaces/w1") ? { my_role: "approver" } : {}), {
        status: 200,
        headers: { "content-type": "application/json" },
      }),
    );
    vi.stubGlobal("fetch", fn);
    const pending = ev(1, "approval.pending", { approval_id: "ap1", summary: "Call example.com" });
    const state = applyEvent(emptyTranscript, pending);
    render(
      <QueryClientProvider client={new QueryClient()}>
        <ApprovalCard ev={pending} state={state} wsId="w1" />
      </QueryClientProvider>,
    );
    fireEvent.click(await screen.findByRole("button", { name: "Approve" }));
    await waitFor(() => expect(fn.mock.calls.some((c) => c[0].url.includes("/decision"))).toBe(true));
    const req = fn.mock.calls.find((c) => c[0].url.includes("/decision")) as unknown as [Request];
    expect(req[0].url).toContain("/v1/approvals/ap1/decision");
    expect(await req[0].clone().json()).toEqual({ decision: "approve" });
  });
  it("shows a plain member a read-only card (RFE-012)", async () => {
    const fn = vi.fn(async () =>
      new Response(JSON.stringify({ my_role: "member" }), { status: 200, headers: { "content-type": "application/json" } }),
    );
    vi.stubGlobal("fetch", fn);
    const pending = ev(1, "approval.pending", { approval_id: "ap1", summary: "Call example.com" });
    const state = applyEvent(emptyTranscript, pending);
    render(
      <QueryClientProvider client={new QueryClient()}>
        <ApprovalCard ev={pending} state={state} wsId="w1" />
      </QueryClientProvider>,
    );
    expect(await screen.findByText("Waiting for an approver")).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Approve" })).toBeNull();
  });
  it("shows the decided state without buttons", () => {
    const pending = ev(1, "approval.pending", { approval_id: "ap1", summary: "s" });
    const state = applyEvents(emptyTranscript, [pending, ev(2, "approval.decided", { approval_id: "ap1", decision: "deny", decided_by_label: "Ann" })]);
    render(
      <QueryClientProvider client={new QueryClient()}>
        <ApprovalCard ev={pending} state={state} wsId="w1" />
      </QueryClientProvider>,
    );
    expect(screen.queryByRole("button", { name: "Approve" })).toBeNull();
    expect(screen.getByText(/Denied by Ann/)).toBeTruthy();
  });
});
