import type { Conversation, TranscriptEvent } from "@/lib/api/conversations";

import { activeTurnId, type PurgeReason, type TranscriptState } from "./transcript";

// Pure helpers behind the chat layout (D-102): sender grouping, day dividers, list ordering and
// filtering, and the folded tool-call summary. No React, no clock: callers pass `now`.

const str = (v: unknown): string => (typeof v === "string" ? v : "");

/** Messages from one sender within this window share one meta line. */
export const GROUP_WINDOW_MS = 5 * 60_000;

const startOfDay = (d: Date) => new Date(d.getFullYear(), d.getMonth(), d.getDate());
const dayDiff = (a: Date, b: Date) => Math.round((startOfDay(a).getTime() - startOfDay(b).getTime()) / 86_400_000);
// en-GB: day before month and a 24-hour clock, matching the dashboard's other dates.
const WEEKDAY = new Intl.DateTimeFormat("en-GB", { weekday: "long" });
const WEEKDAY_SHORT = new Intl.DateTimeFormat("en-GB", { weekday: "short" });
const DAY_MONTH = new Intl.DateTimeFormat("en-GB", { day: "numeric", month: "short" });
const DAY_MONTH_YEAR = new Intl.DateTimeFormat("en-GB", { day: "numeric", month: "short", year: "numeric" });
const CLOCK = new Intl.DateTimeFormat("en-GB", { hour: "2-digit", minute: "2-digit" });

/** `14:05` in the viewer's time zone. */
export function clockTime(iso: string): string {
  const d = new Date(iso);
  return Number.isNaN(d.getTime()) ? "" : CLOCK.format(d);
}

/** Divider label: Today, Yesterday, the weekday within the last week, then "Mon, 3 Nov" (with the year when it differs). */
export function dayLabel(iso: string, now: Date): string {
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return "";
  const diff = dayDiff(now, d);
  if (diff === 0) return "Today";
  if (diff === 1) return "Yesterday";
  if (diff > 1 && diff < 7) return WEEKDAY.format(d);
  const base = `${WEEKDAY_SHORT.format(d)}, ${DAY_MONTH.format(d)}`;
  return d.getFullYear() === now.getFullYear() ? base : `${base} ${d.getFullYear()}`;
}

/** List time: the clock today, the short weekday within the week, a date before that. */
export function listTime(iso: string | null | undefined, now: Date): string {
  if (!iso) return "";
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return "";
  const diff = dayDiff(now, d);
  if (diff <= 0) return CLOCK.format(d);
  if (diff < 7) return WEEKDAY_SHORT.format(d);
  return d.getFullYear() === now.getFullYear() ? DAY_MONTH.format(d) : DAY_MONTH_YEAR.format(d);
}

// ---------------------------------------------------------------------------------- conversations

export const KIND_LABEL: Record<string, string> = {
  direct: "Direct chat",
  group: "Group",
  schedule: "Schedule",
  webhook: "Webhook",
  watch: "Watch",
  peer: "Peer agent",
  api: "API",
};
export const kindLabel = (kind: string): string => KIND_LABEL[kind] ?? kind;

/** The row title: `title`, else "Direct chat" for a direct conversation, else the external reference. */
export function conversationTitle(c: Pick<Conversation, "title" | "kind" | "external_ref" | "direct_user">): string {
  if (c.title) return c.title;
  if (c.kind === "direct") return c.direct_user?.display_name || "Direct chat";
  return c.external_ref;
}

/** A Working dot is shown while the conversation has a turn in flight. */
export const isWorking = (c: Pick<Conversation, "active_turn">): boolean => !!c.active_turn;

const activity = (c: Conversation): number => {
  const t = Date.parse(c.last_activity_at ?? c.created_at);
  return Number.isNaN(t) ? 0 : t;
};

/** Newest activity first (`last_activity_at`, else `created_at`). Does not mutate. */
export function sortByActivity(convs: Conversation[]): Conversation[] {
  return [...convs].sort((a, b) => activity(b) - activity(a));
}

export interface ListGroups {
  /** The caller's own direct conversation, always first. */
  mine: Conversation | null;
  /** Everything else visible to the caller, newest activity first. */
  others: Conversation[];
  /** Other members' direct conversations (owners only), newest activity first. */
  memberDirects: Conversation[];
}

/** Own direct chat first, then `last_activity_at` desc; other members' direct chats are a separate group. */
export function groupConversations(convs: Conversation[], directId: string | null): ListGroups {
  const byActivity = (a: Conversation, b: Conversation) => activity(b) - activity(a);
  const mine = convs.find((c) => c.id === directId) ?? null;
  const rest = convs.filter((c) => c.id !== directId);
  return {
    mine,
    others: rest.filter((c) => !(c.kind === "direct" && c.direct_user)).sort(byActivity),
    memberDirects: rest.filter((c) => c.kind === "direct" && c.direct_user).sort(byActivity),
  };
}

/** Client-side title filter (there is no search API). */
export function filterConversations(convs: Conversation[], query: string): Conversation[] {
  const q = query.trim().toLowerCase();
  if (!q) return convs;
  return convs.filter((c) => conversationTitle(c).toLowerCase().includes(q));
}

// -------------------------------------------------------------------------------------- transcript

export interface ToolCall {
  callId: string;
  name: string;
  summary: string;
  /** `null` while the call runs. */
  ok: boolean | null;
  result: string;
  durationMs: number | null;
  /** Gateway decision when the payload carries one; otherwise `TOOL`. */
  decision: "ALLOW" | "HELD" | "DENY" | "TOOL";
}

export interface ToolSummary {
  count: number;
  totalMs: number | null;
  hasDeny: boolean;
  label: string;
}

export function formatDuration(ms: number): string {
  if (ms < 1000) return `${Math.max(0, Math.round(ms))} ms`;
  const s = ms / 1000;
  return s < 60 ? `${s.toFixed(1)} s` : `${Math.floor(s / 60)} m ${Math.round(s % 60)} s`;
}

/** "Used 3 tools · 4.2 s", with `hasDeny` when any call was denied or failed. */
export function toolGroupSummary(calls: ToolCall[]): ToolSummary {
  const durations = calls.map((c) => c.durationMs).filter((d): d is number => d !== null);
  const totalMs = durations.length > 0 ? durations.reduce((a, b) => a + b, 0) : null;
  const hasDeny = calls.some((c) => c.decision === "DENY" || c.ok === false);
  const used = `Used ${calls.length} ${calls.length === 1 ? "tool" : "tools"}`;
  return { count: calls.length, totalMs, hasDeny, label: totalMs === null ? used : `${used} · ${formatDuration(totalMs)}` };
}

function decisionOf(p: Record<string, unknown>, ok: boolean | null): ToolCall["decision"] {
  const raw = str(p.decision).toLowerCase();
  if (raw === "allow" || raw === "allowed") return "ALLOW";
  if (raw === "held" || raw === "hold") return "HELD";
  if (raw === "deny" || raw === "denied" || raw === "blocked") return "DENY";
  return ok === false && raw ? "DENY" : "TOOL";
}

export type Block =
  | { kind: "bubble"; ev: TranscriptEvent }
  | { kind: "tools"; key: string; turnId: string; calls: ToolCall[]; running: boolean }
  | { kind: "approval"; ev: TranscriptEvent }
  | { kind: "attachment"; ev: TranscriptEvent }
  | { kind: "error"; ev: TranscriptEvent };

export type Row =
  | { kind: "day"; key: string; label: string }
  | {
      kind: "group";
      key: string;
      side: "person" | "agent";
      senderKey: string;
      /** Sender name: the speaker label for people and sources; empty for the agent. */
      name: string;
      door: string;
      ts: string;
      blocks: Block[];
    }
  | { kind: "redacted"; key: string; ev: TranscriptEvent }
  | { kind: "system"; key: string; ev: TranscriptEvent };

/** Events that never render on their own (their effect is folded into another row). */
const FOLDED = new Set(["turn.started", "tool.finished", "approval.decided", "approval.expired", "event.redacted"]);

/** Turn the durable events into day dividers, sender groups (5-minute window), folded tool calls and standalone rows. */
export function buildRows(state: TranscriptState, now: Date): Row[] {
  const rows: Row[] = [];
  const running = new Set<string>();
  const open = activeTurnId(state);
  if (open) running.add(open);
  const finishedAt = new Map<string, { ts: string; ok: boolean; summary: string }>();
  for (const e of state.events) {
    if (e.type === "tool.finished") finishedAt.set(str(e.payload.call_id), { ts: e.ts, ok: e.payload.ok === true, summary: str(e.payload.summary) });
  }
  // A failed turn.finished repeats the `error` event's code: render the error once.
  const errored = new Set(state.events.filter((e) => e.type === "error").map((e) => str(e.payload.turn_id)));
  let lastDay = "";
  // The open group; a holder object because `push` assigns it from a closure.
  const cur: { group: Extract<Row, { kind: "group" }> | null } = { group: null };
  let lastTs = 0;

  const push = (side: "person" | "agent", senderKey: string, name: string, door: string, ev: TranscriptEvent, block: Block) => {
    const t = Date.parse(ev.ts);
    const open = cur.group;
    if (!open || open.senderKey !== senderKey || Number.isNaN(t) || t - lastTs > GROUP_WINDOW_MS || t < lastTs) {
      cur.group = { kind: "group", key: `g${ev.seq}`, side, senderKey, name, door, ts: ev.ts, blocks: [block] };
      rows.push(cur.group);
    } else {
      open.blocks.push(block);
    }
    if (!Number.isNaN(t)) lastTs = t;
  };

  for (const ev of state.events) {
    if (FOLDED.has(ev.type)) continue;
    const p = ev.payload;
    const day = new Date(ev.ts).toDateString();
    if (day !== lastDay) {
      lastDay = day;
      cur.group = null;
      const label = dayLabel(ev.ts, now);
      if (label) rows.push({ kind: "day", key: `d${ev.seq}`, label });
    }
    if (p.redacted === true) {
      cur.group = null;
      rows.push({ kind: "redacted", key: `r${ev.seq}`, ev });
      continue;
    }
    switch (ev.type) {
      case "turn.queued": {
        const door = str(p.door);
        const name = str(p.speaker_label) || "You";
        push("person", `p:${name}`, name, door && door !== "dashboard" ? door : "", ev, { kind: "bubble", ev });
        break;
      }
      case "text":
        push("agent", "agent", "", "", ev, { kind: "bubble", ev });
        break;
      case "tool.started": {
        const fin = finishedAt.get(str(p.call_id));
        const ok = fin ? fin.ok : null;
        const dur = fin ? Date.parse(fin.ts) - Date.parse(ev.ts) : NaN;
        const call: ToolCall = {
          callId: str(p.call_id),
          name: str(p.name),
          summary: str(p.summary),
          ok,
          result: fin?.summary ?? "",
          durationMs: Number.isNaN(dur) ? null : Math.max(0, dur),
          decision: decisionOf(p, ok),
        };
        const turnId = str(p.turn_id);
        const g = cur.group;
        const prev: Block | undefined = g?.senderKey === "agent" ? g.blocks[g.blocks.length - 1] : undefined;
        if (prev && prev.kind === "tools" && prev.turnId === turnId) {
          prev.calls.push(call);
          const t = Date.parse(ev.ts);
          if (!Number.isNaN(t)) lastTs = t;
        } else {
          push("agent", "agent", "", "", ev, { kind: "tools", key: `t${ev.seq}`, turnId, calls: [call], running: running.has(turnId) });
        }
        break;
      }
      case "approval.pending":
      case "action_approval.pending":
        push("agent", "agent", "", "", ev, { kind: "approval", ev });
        break;
      case "attachment":
        push("agent", "agent", "", "", ev, { kind: "attachment", ev });
        break;
      case "error":
        push("agent", "agent", "", "", ev, { kind: "error", ev });
        break;
      case "turn.finished":
        if (str(p.status) === "failed" && str(p.error_code) && !errored.has(str(p.turn_id))) push("agent", "agent", "", "", ev, { kind: "error", ev });
        break;
      case "notice":
      case "correction":
      case "message.joined":
      case "message.sent_now":
        cur.group = null;
        rows.push({ kind: "system", key: `s${ev.seq}`, ev });
        break;
      default:
        break; // unknown types are not rendered
    }
  }
  return rows;
}

/** The purge reason of an attachment id, for the "Expired" chip. */
export const purgedReason = (state: TranscriptState, id: string): PurgeReason | undefined => state.purged?.[id];
