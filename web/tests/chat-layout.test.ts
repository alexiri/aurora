import { describe, expect, it } from "vitest";

import type { Conversation, TranscriptEvent } from "@/lib/api/conversations";
import {
  buildRows,
  conversationTitle,
  dayLabel,
  filterConversations,
  groupConversations,
  isWorking,
  listTime,
  toolGroupSummary,
  type Row,
  type ToolCall,
} from "@/lib/chat/layout";
import { applyEvents, emptyTranscript } from "@/lib/chat/transcript";

// Local-time instants, so the day maths holds in any time zone.
const at = (y: number, mo: number, d: number, h = 12, mi = 0, s = 0) => new Date(y, mo - 1, d, h, mi, s).toISOString();
const NOW = new Date(2026, 10, 8, 15, 0, 0); // Sunday 8 Nov 2026

let n = 0;
const ev = (type: string, ts: string, payload: Record<string, unknown>): TranscriptEvent => ({ seq: ++n, type, ts, payload });
const rows = (events: TranscriptEvent[]) => buildRows(applyEvents(emptyTranscript, events), NOW);
const groups = (r: Row[]) => r.filter((x): x is Extract<Row, { kind: "group" }> => x.kind === "group");

describe("sender grouping", () => {
  it("shares one group for the same sender within 5 minutes", () => {
    const g = groups(
      rows([
        ev("turn.queued", at(2026, 11, 8, 12, 0), { turn_id: "t1", speaker_label: "Ann", text: "a" }),
        ev("turn.queued", at(2026, 11, 8, 12, 4, 59), { turn_id: "t2", speaker_label: "Ann", text: "b" }),
      ]),
    );
    expect(g).toHaveLength(1);
    expect(g[0].blocks).toHaveLength(2);
  });

  it("starts a new group after more than 5 minutes", () => {
    const g = groups(
      rows([
        ev("turn.queued", at(2026, 11, 8, 12, 0), { turn_id: "t1", speaker_label: "Ann", text: "a" }),
        ev("turn.queued", at(2026, 11, 8, 12, 5, 1), { turn_id: "t2", speaker_label: "Ann", text: "b" }),
      ]),
    );
    expect(g).toHaveLength(2);
  });

  it("starts a new group when the sender changes, and shows the door of the source", () => {
    const g = groups(
      rows([
        ev("turn.queued", at(2026, 11, 8, 12, 0), { turn_id: "t1", speaker_label: "Ann", door: "dashboard", text: "a" }),
        ev("text", at(2026, 11, 8, 12, 1), { turn_id: "t1", message_id: "m1", text: "hi" }),
        ev("turn.queued", at(2026, 11, 8, 12, 2), { turn_id: "t2", speaker_label: "Nightly report", door: "schedule", text: "b" }),
      ]),
    );
    expect(g.map((x) => x.side)).toEqual(["person", "agent", "person"]);
    expect(g[0].door).toBe("");
    expect(g[2].name).toBe("Nightly report");
    expect(g[2].door).toBe("schedule");
  });

  it("keeps an agent's text, tool calls and approval in one group", () => {
    const g = groups(
      rows([
        ev("turn.queued", at(2026, 11, 8, 12, 0), { turn_id: "t1", text: "go" }),
        ev("tool.started", at(2026, 11, 8, 12, 0, 1), { turn_id: "t1", call_id: "c1", name: "read_file", summary: "report.md" }),
        ev("tool.started", at(2026, 11, 8, 12, 0, 2), { turn_id: "t1", call_id: "c2", name: "github", summary: "GET /repos" }),
        ev("approval.pending", at(2026, 11, 8, 12, 0, 3), { turn_id: "t1", approval_id: "a1", summary: "Merge" }),
      ]),
    );
    expect(g).toHaveLength(2);
    expect(g[1].blocks.map((b) => b.kind)).toEqual(["tools", "approval"]);
    const tools = g[1].blocks[0];
    expect(tools.kind === "tools" && tools.calls).toHaveLength(2);
  });

  it("a redacted event is its own row and breaks the group", () => {
    const r = rows([
      ev("turn.queued", at(2026, 11, 8, 12, 0), { turn_id: "t1", speaker_label: "Ann", text: "a" }),
      ev("turn.queued", at(2026, 11, 8, 12, 1), { turn_id: "t2", speaker_label: "Ann", redacted: true }),
      ev("turn.queued", at(2026, 11, 8, 12, 2), { turn_id: "t3", speaker_label: "Ann", text: "c" }),
    ]);
    expect(r.map((x) => x.kind)).toEqual(["day", "group", "redacted", "group"]);
  });
});

describe("day dividers", () => {
  it("labels Today, Yesterday, weekday, then the date", () => {
    expect(dayLabel(at(2026, 11, 8, 9), NOW)).toBe("Today");
    expect(dayLabel(at(2026, 11, 7, 9), NOW)).toBe("Yesterday");
    expect(dayLabel(at(2026, 11, 4, 9), NOW)).toBe("Wednesday");
    expect(dayLabel(at(2026, 10, 20, 9), NOW)).toBe("Tue, 20 Oct");
    expect(dayLabel(at(2025, 12, 31, 9), NOW)).toBe("Wed, 31 Dec 2025");
  });

  it("inserts a divider whenever the day changes and breaks the group", () => {
    const r = rows([
      ev("turn.queued", at(2026, 11, 7, 23, 59), { turn_id: "t1", speaker_label: "Ann", text: "a" }),
      ev("turn.queued", at(2026, 11, 8, 0, 1), { turn_id: "t2", speaker_label: "Ann", text: "b" }),
    ]);
    expect(r.map((x) => (x.kind === "day" ? x.label : x.kind))).toEqual(["Yesterday", "group", "Today", "group"]);
  });

  it("formats the list time: clock today, weekday this week, date before", () => {
    expect(listTime(at(2026, 11, 8, 9, 5), NOW)).toBe("09:05");
    expect(listTime(at(2026, 11, 5, 9, 5), NOW)).toBe("Thu");
    expect(listTime(at(2026, 10, 1, 9, 5), NOW)).toBe("1 Oct");
    expect(listTime(null, NOW)).toBe("");
  });
});

const conv = (id: string, over: Partial<Conversation> = {}): Conversation =>
  ({
    id, agent_id: "a", kind: "group", external_ref: `ref-${id}`, title: null, direct_user_id: null, direct_user: null, origin_conversation_id: null,
    last_seq: 0, app_turn_streak: 0, app_turn_capped: false, active_turn: null, last_activity_at: null, created_at: at(2026, 1, 1), ...over,
  }) as Conversation;

describe("conversation list", () => {
  it("puts the own direct chat first, then the rest by last_activity_at desc", () => {
    const g = groupConversations(
      [
        conv("old", { last_activity_at: at(2026, 11, 1) }),
        conv("mine", { kind: "direct", last_activity_at: at(2026, 10, 1) }),
        conv("new", { last_activity_at: at(2026, 11, 7) }),
        conv("none"),
      ],
      "mine",
    );
    expect(g.mine?.id).toBe("mine");
    expect(g.others.map((c) => c.id)).toEqual(["new", "old", "none"]);
  });

  it("keeps other members' direct chats in their own group", () => {
    const g = groupConversations(
      [conv("mine", { kind: "direct" }), conv("ann", { kind: "direct", direct_user: { id: "u", display_name: "Ann" } as never })],
      "mine",
    );
    expect(g.memberDirects.map((c) => c.id)).toEqual(["ann"]);
    expect(g.others).toEqual([]);
  });

  it("filters by title on the client, case-insensitively", () => {
    const all = [conv("1", { title: "Team room" }), conv("2", { title: "Release notes" }), conv("3", { kind: "direct" })];
    expect(filterConversations(all, "ROOM").map((c) => c.id)).toEqual(["1"]);
    expect(filterConversations(all, "direct").map((c) => c.id)).toEqual(["3"]);
    expect(filterConversations(all, "  ")).toHaveLength(3);
    expect(conversationTitle(all[2])).toBe("Direct chat");
  });

  it("shows the Working dot only while a turn is active", () => {
    expect(isWorking(conv("1"))).toBe(false);
    expect(isWorking(conv("2", { active_turn: { id: "t" } as never }))).toBe(true);
  });
});

describe("tool call group summary", () => {
  const call = (over: Partial<ToolCall> = {}): ToolCall => ({ callId: "c", name: "n", summary: "", ok: true, result: "", durationMs: 1000, decision: "TOOL", ...over });

  it("counts the calls and adds up the durations", () => {
    const s = toolGroupSummary([call({ durationMs: 1200 }), call({ durationMs: 3000 })]);
    expect(s).toMatchObject({ count: 2, totalMs: 4200, hasDeny: false, label: "Used 2 tools · 4.2 s" });
    expect(toolGroupSummary([call({ durationMs: 40 })]).label).toBe("Used 1 tool · 40 ms");
  });

  it("omits the duration while unknown", () => {
    expect(toolGroupSummary([call({ ok: null, durationMs: null })]).label).toBe("Used 1 tool");
  });

  it("flags DENY when a call was denied or failed", () => {
    expect(toolGroupSummary([call(), call({ decision: "DENY" })]).hasDeny).toBe(true);
    expect(toolGroupSummary([call(), call({ ok: false })]).hasDeny).toBe(true);
  });

  it("builds durations and results from tool.started and tool.finished", () => {
    const r = rows([
      ev("tool.started", at(2026, 11, 8, 12, 0, 0), { turn_id: "t1", call_id: "c1", name: "read_file", summary: "report.md" }),
      ev("tool.finished", at(2026, 11, 8, 12, 0, 2), { turn_id: "t1", call_id: "c1", ok: true, summary: "12 KiB" }),
      ev("tool.started", at(2026, 11, 8, 12, 0, 3), { turn_id: "t1", call_id: "c2", name: "write_file", summary: "out.md" }),
      ev("tool.finished", at(2026, 11, 8, 12, 0, 4), { turn_id: "t1", call_id: "c2", ok: false, summary: "denied" }),
    ]);
    const block = groups(r)[0].blocks[0];
    if (block.kind !== "tools") throw new Error("expected a tools block");
    expect(block.calls.map((c) => c.durationMs)).toEqual([2000, 1000]);
    expect(toolGroupSummary(block.calls)).toMatchObject({ label: "Used 2 tools · 3.0 s", hasDeny: true });
  });
});
