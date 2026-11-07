import type { Conversation, Followup, SendResult, TranscriptEvent } from "@/lib/api/conversations";

export interface Delta {
  turnId: string;
  messageId: string;
  text: string;
}

export interface TranscriptState {
  /** Durable events ordered by seq, deduplicated. */
  events: TranscriptEvent[];
  lastSeq: number;
  /** Ephemeral partial answers keyed by message id. */
  deltas: Record<string, Delta>;
  /** Latest activity line per turn. */
  activity: Record<string, string>;
}

export const emptyTranscript: TranscriptState = { events: [], lastSeq: 0, deltas: {}, activity: {} };

const str = (v: unknown): string => (typeof v === "string" ? v : "");

/** Apply one event (history or stream). Unknown types are ignored; durable events dedupe by seq. */
export function applyEvent(state: TranscriptState, ev: TranscriptEvent): TranscriptState {
  const p = ev.payload ?? {};
  if (ev.seq === null || ev.seq === undefined) {
    if (ev.type === "text.delta") {
      const messageId = str(p.message_id);
      if (!messageId) return state;
      const prev = state.deltas[messageId];
      return {
        ...state,
        deltas: { ...state.deltas, [messageId]: { turnId: str(p.turn_id), messageId, text: (prev?.text ?? "") + str(p.delta) } },
      };
    }
    if (ev.type === "activity") {
      return { ...state, activity: { ...state.activity, [str(p.turn_id)]: str(p.line) } };
    }
    return state; // thinking.delta and unknown ephemerals are not rendered
  }
  // A redacted event keeps its seq; a refreshed copy replaces the stale one (RFE-010).
  if (state.events.some((e) => e.seq === ev.seq)) {
    const stale = state.events.find((e) => e.seq === ev.seq);
    if (stale && p.redacted === true && stale.payload.redacted !== true) {
      return { ...state, events: state.events.map((e) => (e.seq === ev.seq ? ev : e)) };
    }
    return state;
  }
  const events = [...state.events, ev].sort((a, b) => (a.seq as number) - (b.seq as number));
  let { deltas, activity } = state;
  if (ev.type === "text") {
    const rest = { ...deltas };
    delete rest[str(p.message_id)];
    deltas = rest;
  } else if (ev.type === "turn.finished") {
    const turnId = str(p.turn_id);
    deltas = Object.fromEntries(Object.entries(deltas).filter(([, d]) => d.turnId !== turnId));
    const a = { ...activity };
    delete a[turnId];
    activity = a;
  }
  return { events, lastSeq: Math.max(state.lastSeq, ev.seq), deltas, activity };
}

export function applyEvents(state: TranscriptState, evs: TranscriptEvent[]): TranscriptState {
  return evs.reduce(applyEvent, state);
}

/** Resume: fetch every durable event after `afterSeq`, paging until exhausted. */
export async function loadHistory(
  fetchPage: (afterSeq: number) => Promise<{ items: TranscriptEvent[]; last_seq: number }>,
  state: TranscriptState = emptyTranscript,
): Promise<TranscriptState> {
  let s = state;
  for (let guard = 0; guard < 1000; guard++) {
    const page = await fetchPage(s.lastSeq);
    if (page.items.length === 0) break;
    const before = s.lastSeq;
    s = applyEvents(s, page.items);
    if (s.lastSeq <= before || s.lastSeq >= page.last_seq) break;
  }
  return s;
}

export const STREAM_EVENT_TYPES = [
  "turn.queued", "turn.started", "text.delta", "thinking.delta", "activity", "text",
  "tool.started", "tool.finished", "correction", "message.joined", "message.sent_now",
  "approval.pending", "approval.decided", "approval.expired",
  "action_approval.pending", "action_approval.decided", "attachment", "notice", "error", "turn.finished",
  "event.redacted",
] as const;

export function parseStreamEvent(raw: string): TranscriptEvent | null {
  try {
    const v = JSON.parse(raw) as TranscriptEvent;
    if (v && typeof v.type === "string" && typeof v.payload === "object") return v;
  } catch {
    /* ignore malformed frames */
  }
  return null;
}

/** Id of the turn currently in flight (queued/started without turn.finished), or the conversation's active turn. */
export function activeTurnId(state: TranscriptState, conv?: Pick<Conversation, "active_turn"> | null): string | null {
  const open: string[] = [];
  for (const e of state.events) {
    const id = str(e.payload.turn_id);
    if (!id) continue;
    if (e.type === "turn.queued" || e.type === "turn.started") {
      if (!open.includes(id)) open.push(id);
    } else if (e.type === "turn.finished") {
      const i = open.indexOf(id);
      if (i >= 0) open.splice(i, 1);
    }
  }
  if (open.length) return open[open.length - 1];
  if (state.events.length === 0 && conv?.active_turn) return conv.active_turn.id;
  return null;
}

/** Composer outcome of POST /messages (C-25: exactly one of turn / followup). */
export type SendOutcome = { kind: "turn"; turnId: string } | { kind: "followup"; followup: Followup } | { kind: "unknown" };

export function classifySend(res: SendResult): SendOutcome {
  if (res.turn) return { kind: "turn", turnId: res.turn.id };
  if (res.followup) return { kind: "followup", followup: res.followup };
  return { kind: "unknown" };
}

/** Followups still waiting: drop those whose parent turn finished or was sent now. */
export function pendingFollowups(followups: Followup[], state: TranscriptState): Followup[] {
  const done = new Set<string>();
  for (const e of state.events) {
    if (e.type === "turn.finished" || e.type === "message.sent_now") done.add(str(e.payload.turn_id));
  }
  return followups.filter((f) => f.status !== "promoted" && !done.has(f.parent_turn_id));
}

/** Approval status derived from the durable decided/expired events. */
export function approvalStatus(state: TranscriptState, approvalId: string): { status: "pending" | "approve" | "deny" | "expired"; by?: string } {
  for (const e of state.events) {
    if (str(e.payload.approval_id) !== approvalId) continue;
    if (e.type === "approval.expired") return { status: "expired" };
    if (e.type === "approval.decided") {
      const d = str(e.payload.decision);
      return { status: d === "deny" || d === "denied" ? "deny" : "approve", by: str(e.payload.decided_by_label) || undefined };
    }
  }
  return { status: "pending" };
}

export interface ErrorCopy {
  message: string;
  link?: { label: string; path: "credentials" | "computer" };
}

const ERROR_COPY: Record<string, ErrorCopy> = {
  no_model_key: { message: "This agent has no usable LLM key.", link: { label: "Add an LLM key", path: "credentials" } },
  model_not_allowed: { message: "The selected model is not allowed. Choose another model." },
  hosted_disabled: { message: "Hosted agents are not enabled on this instance." },
  turn_finished: { message: "That turn has already finished." },
  sandbox_failed: { message: "The agent's computer failed to start.", link: { label: "Open Computer tab", path: "computer" } },
  start_failed: { message: "The agent's computer failed to start.", link: { label: "Open Computer tab", path: "computer" } },
};

/** Known code → copy (+ deep link); unknown → generic. */
export function errorCopy(code: string | null | undefined, fallback?: string): ErrorCopy {
  if (code && ERROR_COPY[code]) return ERROR_COPY[code];
  return { message: fallback ? `Something went wrong: ${fallback}` : "Something went wrong. Please try again." };
}
