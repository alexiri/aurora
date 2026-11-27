import type { Conversation, TranscriptEvent } from "@/lib/api/conversations";
import { displayTimeZone } from "@/lib/format-date";

import { personName } from "@/lib/workspace-name";
import { activeTurnId, type TranscriptState } from "./transcript";

// Pure helpers behind the chat layout (D-102): sender grouping, day dividers, list ordering and
// filtering, and the folded tool-call summary. No React, no clock: callers pass `now`.

const str = (v: unknown): string => (typeof v === "string" ? v : "");

/** Messages from one sender within this window share one meta line. */
export const GROUP_WINDOW_MS = 5 * 60_000;

// (P5-REFINE-20, b2#18) One zone on a screen: every stamp, day divider and list time is in the person's zone
// (`displayTimeZone()`, the browser's when none is set), like `formatWhen`.
const formatters = new Map<string, Intl.DateTimeFormat>();
/** A cached en-GB formatter (day before month, 24-hour clock, matching the dashboard's other dates) in the display zone. */
function fmt(opts: Intl.DateTimeFormatOptions): Intl.DateTimeFormat {
  const timeZone = displayTimeZone();
  const key = `${timeZone ?? ""}|${JSON.stringify(opts)}`;
  let f = formatters.get(key);
  if (!f) {
    f = new Intl.DateTimeFormat("en-GB", { ...opts, timeZone });
    formatters.set(key, f);
  }
  return f;
}
/** The calendar day of `d` in the display zone, as a day number. */
function dayIndex(d: Date): number {
  const p = Object.fromEntries(fmt({ year: "numeric", month: "numeric", day: "numeric" }).formatToParts(d).map((x) => [x.type, x.value]));
  return Date.UTC(Number(p.year), Number(p.month) - 1, Number(p.day)) / 86_400_000;
}
const yearIn = (d: Date) => Number(fmt({ year: "numeric" }).format(d));
const dayDiff = (a: Date, b: Date) => Math.round(dayIndex(a) - dayIndex(b));
const WEEKDAY = { format: (d: Date) => fmt({ weekday: "long" }).format(d) };
const WEEKDAY_SHORT = { format: (d: Date) => fmt({ weekday: "short" }).format(d) };
const DAY_MONTH = { format: (d: Date) => fmt({ day: "numeric", month: "short" }).format(d) };
const DAY_MONTH_YEAR = { format: (d: Date) => fmt({ day: "numeric", month: "short", year: "numeric" }).format(d) };
const CLOCK = { format: (d: Date) => fmt({ hour: "2-digit", minute: "2-digit" }).format(d) };

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
  return yearIn(d) === yearIn(now) ? base : `${base} ${yearIn(d)}`;
}

/** List time: the clock today, the short weekday within the week, a date before that. */
export function listTime(iso: string | null | undefined, now: Date): string {
  if (!iso) return "";
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return "";
  const diff = dayDiff(now, d);
  if (diff <= 0) return CLOCK.format(d);
  if (diff < 7) return WEEKDAY_SHORT.format(d);
  return yearIn(d) === yearIn(now) ? DAY_MONTH.format(d) : DAY_MONTH_YEAR.format(d);
}

/** (chat-9) A search result's time: the date and the clock, "19 Nov 23:50" ("19 Nov 2025 23:50" in another year). */
export function searchTime(iso: string | null | undefined, now: Date, timeZone: string | undefined = displayTimeZone()): string {
  if (!iso) return "";
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return "";
  const parts = (o: Intl.DateTimeFormatOptions) => new Intl.DateTimeFormat("en-GB", { timeZone, ...o }).format(d);
  const sameYear = yearIn(d) === yearIn(now);
  return `${parts({ day: "numeric", month: "short", ...(sameYear ? {} : { year: "numeric" }) })} ${parts({ hour: "2-digit", minute: "2-digit", hour12: false })}`;
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

/**
 * (schedules-7, C-1098, C-1074) What a run thread's header says about its automation after the kind: the first rename note
 * `was "Every minute ping", renamed with it`, then "Schedule deleted" / "Webhook deleted" once it is gone.
 */
export function automationNotes(c: Partial<Pick<Conversation, "automation">>): string[] {
  const a = c.automation;
  if (!a) return [];
  return [...(a.previous_name ? [`was "${a.previous_name}", renamed with it`] : []), ...(a.deleted ? [a.kind === "webhook" ? "Webhook deleted" : "Schedule deleted"] : [])];
}

/**
 * The row title: a run thread of an automation is "Morning summary · 19 Nov 09:00" (D-409, C-1074); otherwise `title`,
 * else "Direct chat" for a direct conversation, else the external reference.
 */
export function conversationTitle(
  c: Pick<Conversation, "title" | "kind" | "external_ref" | "direct_user"> & Partial<Pick<Conversation, "automation" | "run_at">>,
  now: Date = new Date(),
): string {
  if (c.automation && c.run_at) {
    // A run's thread is named in the person's zone, like every other time on the screen.
    const when = searchTime(c.run_at, now);
    if (when) return `${c.automation.name} · ${when}`;
  }
  if (c.title) return c.title;
  if (c.kind === "direct") return c.direct_user?.display_name ? personName(c.direct_user.display_name) : "Direct chat";
  return c.external_ref;
}

/**
 * A Working dot is shown while the conversation has a turn in flight. A turn the person pressed Stop on (`stopped`,
 * `lib/chat/stopped.ts`) or whose abort was requested is not working any more, though the list has not refetched
 * (P5-REFINE-11, chat-4).
 */
export const isWorking = (c: Pick<Conversation, "active_turn">, stopped?: ReadonlySet<string>): boolean =>
  !!c.active_turn && !c.active_turn.abort_requested && !stopped?.has(c.active_turn.id);

const activity = (c: Conversation): number => {
  const t = Date.parse(c.last_activity_at ?? c.created_at);
  return Number.isNaN(t) ? 0 : t;
};

/** Newest activity first (`last_activity_at`, else `created_at`). Does not mutate. */
export function sortByActivity(convs: Conversation[]): Conversation[] {
  return [...convs].sort((a, b) => activity(b) - activity(a));
}

/** The caller's own direct chat: a direct conversation the API does not mark with another person (`direct_user`, C-1073). */
export const isOwnDirect = (c: Pick<Conversation, "kind" | "direct_user">): boolean => c.kind === "direct" && !c.direct_user;

export interface ListGroups {
  /** (D-409) The caller's own direct chats, newest activity first: the person can have several. */
  mine: Conversation[];
  /** Everything else visible to the caller, newest activity first. */
  others: Conversation[];
  /** Other members' direct conversations (owners only), newest activity first. */
  memberDirects: Conversation[];
}

/** Own direct chats first (newest first), then `last_activity_at` desc; other members' direct chats are a separate group. */
export function groupConversations(convs: Conversation[]): ListGroups {
  const byActivity = (a: Conversation, b: Conversation) => activity(b) - activity(a);
  return {
    mine: convs.filter(isOwnDirect).sort(byActivity),
    others: convs.filter((c) => c.kind !== "direct").sort(byActivity),
    memberDirects: convs.filter((c) => c.kind === "direct" && c.direct_user).sort(byActivity),
  };
}

/** (C-1074) The folder's line: "Morning summary · 3 runs". */
export const folderRunsLabel = (name: string, count: number): string => `${name} · ${count} ${count === 1 ? "run" : "runs"}`;

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
  /** A policy denied a call. */
  hasDeny: boolean;
  /** A call failed for any other reason (nobody denied it). */
  hasFailed: boolean;
  label: string;
}

export function formatDuration(ms: number): string {
  if (ms < 1000) return `${Math.max(0, Math.round(ms))} ms`;
  const s = ms / 1000;
  return s < 60 ? `${s.toFixed(1)} s` : `${Math.floor(s / 60)} m ${Math.round(s % 60)} s`;
}

/** "Used 3 tools · 4.2 s", with `hasDeny` when a policy denied a call and `hasFailed` when a call failed. */
export function toolGroupSummary(calls: ToolCall[]): ToolSummary {
  const durations = calls.map((c) => c.durationMs).filter((d): d is number => d !== null);
  const totalMs = durations.length > 0 ? durations.reduce((a, b) => a + b, 0) : null;
  const hasDeny = calls.some((c) => c.decision === "DENY");
  const hasFailed = calls.some((c) => c.ok === false && c.decision !== "DENY");
  const used = `Used ${calls.length} ${calls.length === 1 ? "tool" : "tools"}`;
  return { count: calls.length, totalMs, hasDeny, hasFailed, label: totalMs === null ? used : `${used} · ${formatDuration(totalMs)}` };
}

/** Only a decision the payload states is shown: a failed call is not a denied one. */
function decisionOf(p: Record<string, unknown>): ToolCall["decision"] {
  const raw = str(p.decision).toLowerCase();
  if (raw === "allow" || raw === "allowed") return "ALLOW";
  if (raw === "held" || raw === "hold") return "HELD";
  if (raw === "deny" || raw === "denied" || raw === "blocked") return "DENY";
  return "TOOL";
}

export type Block =
  | { kind: "bubble"; ev: TranscriptEvent }
  | { kind: "tools"; key: string; turnId: string; calls: ToolCall[]; running: boolean }
  | { kind: "after_turn"; key: string; turnId: string; texts: string[]; calls: ToolCall[] }
  | { kind: "approval"; ev: TranscriptEvent }
  | { kind: "attachment"; ev: TranscriptEvent }
  /** `memory.saved` / `memory.refused` (C-1172): the card under the tool calls of its turn. */
  | { kind: "memory"; ev: TranscriptEvent }
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
const FOLDED = new Set([
  "turn.started",
  "tool.finished",
  "approval.decided",
  "approval.expired",
  "event.redacted",
  // (FIX-225) Asking to steer and editing show on the queued row and the message itself.
  "followup.steer_requested",
  "followup.edited",
]);

const isMemoryOf = (b: Block, turnId: string): boolean => b.kind === "memory" && str(b.ev.payload.turn_id) === turnId;

/** Turn the durable events into day dividers, sender groups (5-minute window), folded tool calls and standalone rows. */
export function buildRows(state: TranscriptState, now: Date): Row[] {
  const rows: Row[] = [];
  const running = new Set<string>();
  const open = activeTurnId(state);
  if (open) running.add(open);
  const finishedAt = new Map<string, { ts: string; ok: boolean; summary: string }>();
  const finishedAfter = new Map<string, { ok: boolean; summary: string }>();
  for (const e of state.events) {
    if (e.type === "tool.finished") finishedAt.set(str(e.payload.call_id), { ts: e.ts, ok: e.payload.ok === true, summary: str(e.payload.summary) });
    // (FIX-253, part B) The idle child's own tool results fold the same way, under "Worked after the turn".
    if (e.type === "after_turn.tool_end") finishedAfter.set(str(e.payload.call_id), { ok: e.payload.ok === true, summary: str(e.payload.summary) });
  }
  // A failed turn.finished repeats the `error` event's code: render the error once.
  const errored = new Set(state.events.filter((e) => e.type === "error").map((e) => str(e.payload.turn_id)));
  // (chat-7) The agent's own bubble that says the same thing as the error ("API Error: 400 No connected db.") is not rendered:
  // the error block shows it once, under Details.
  const errorText = new Map<string, string[]>();
  for (const e of state.events) {
    if (e.type !== "error") continue;
    const id = str(e.payload.turn_id);
    errorText.set(id, [...(errorText.get(id) ?? []), str(e.payload.message).trim(), str(e.payload.detail).trim()].filter(Boolean));
  }
  const repeatsError = (turnId: string, text: string) => {
    const t = text.trim();
    return t !== "" && (errorText.get(turnId) ?? []).some((m) => m === t || m.includes(t) || t.includes(m));
  };
  // (FIX-225) The latest edited text per queued message (`followup.edited`, keyed by the follow-up's id).
  const editedText = new Map<string, string>();
  for (const e of state.events) {
    if (e.type === "followup.edited" && e.payload.redacted !== true) editedText.set(str(e.payload.followup_id), str(e.payload.text));
  }
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
        const label = str(p.speaker_label);
        // (P4-011B-4, C-824, C-836) The resume follow-up (door `system`, ref `approval:<id>`, speaker "Approval"): the
        // transcript event carries the door's name, so the chip is read from that speaker label, shown as "Approval".
        const approval = door === "system" && label === "Approval";
        const name = approval ? "Aurora" : label || "You";
        // A door that is the sender's own name ("Webhook" by `webhook`) is not said twice.
        const chip = approval ? "Approval" : door && door !== "dashboard" && door.toLowerCase() !== name.toLowerCase() ? door : "";
        // (FIX-225) A queued message its author edited reads as it stands now; the event log keeps what was sent.
        const edited = editedText.get(str(p.turn_id));
        push("person", `p:${name}`, name, chip, ev, { kind: "bubble", ev: edited === undefined ? ev : { ...ev, payload: { ...p, text: edited } } });
        break;
      }
      case "text":
        if (repeatsError(str(p.turn_id), str(p.text))) break;
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
          decision: decisionOf(p),
        };
        const turnId = str(p.turn_id);
        const g = cur.group;
        // The turn's memory cards sit under its tool group, so the group is looked for past them. The card's event is
        // written by the API and the call by the runner, so a card can also arrive before its own call: the group
        // then goes in front of it.
        let at = g?.senderKey === "agent" ? g.blocks.length : 0;
        while (g && at > 0 && isMemoryOf(g.blocks[at - 1]!, turnId)) at -= 1;
        const prev: Block | undefined = g?.senderKey === "agent" ? g.blocks[at - 1] : undefined;
        const block: Block = { kind: "tools", key: `t${ev.seq}`, turnId, calls: [call], running: running.has(turnId) };
        if (prev && prev.kind === "tools" && prev.turnId === turnId) {
          prev.calls.push(call);
        } else if (g?.senderKey === "agent" && at < g.blocks.length) {
          g.blocks.splice(at, 0, block);
        } else {
          push("agent", "agent", "", "", ev, block);
          break;
        }
        const t = Date.parse(ev.ts);
        if (!Number.isNaN(t)) lastTs = t;
        break;
      }
      case "memory.saved":
      case "memory.refused":
        push("agent", "agent", "", "", ev, { kind: "memory", ev });
        break;
      case "approval.pending":
      case "action_approval.pending":
        push("agent", "agent", "", "", ev, { kind: "approval", ev });
        break;
      // (FIX-253, part B) The idle child's own run, folded under "Worked after the turn": one
      // block per run, texts and tool calls together.
      case "after_turn.text": {
        const text = str(p.text);
        if (!text) break;
        const turnId = str(p.turn_id);
        const g = cur.group;
        const prev: Block | undefined = g?.senderKey === "agent" ? g.blocks[g.blocks.length - 1] : undefined;
        if (prev && prev.kind === "after_turn" && prev.turnId === turnId) {
          prev.texts.push(text);
        } else {
          push("agent", "agent", "", "", ev, { kind: "after_turn", key: `a${ev.seq}`, turnId, texts: [text], calls: [] });
        }
        break;
      }
      case "after_turn.tool": {
        const fin = finishedAfter.get(str(p.call_id));
        const ok = fin ? fin.ok : null;
        const call: ToolCall = {
          callId: str(p.call_id),
          name: str(p.name),
          summary: str(p.summary),
          ok,
          result: fin?.summary ?? "",
          durationMs: null,
          decision: "TOOL",
        };
        const turnId = str(p.turn_id);
        const g = cur.group;
        const prev: Block | undefined = g?.senderKey === "agent" ? g.blocks[g.blocks.length - 1] : undefined;
        if (prev && prev.kind === "after_turn" && prev.turnId === turnId) {
          prev.calls.push(call);
        } else {
          push("agent", "agent", "", "", ev, { kind: "after_turn", key: `a${ev.seq}`, turnId, texts: [], calls: [call] });
        }
        break;
      }
      case "after_turn.tool_end":
        break; // folded into the call by `finishedAfter` above
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
      // (FIX-225, D-470) What a control did when it did not do what it promised, and a withdrawn message.
      case "followup.cancelled":
      case "followup.steer_failed":
      case "turn.send_now_failed":
        cur.group = null;
        rows.push({ kind: "system", key: `s${ev.seq}`, ev });
        break;
      default:
        break; // unknown types are not rendered
    }
  }
  return rows;
}
