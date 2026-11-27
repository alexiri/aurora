"use client";

import { useEffect, useLayoutEffect, useMemo, useRef, useState, type ReactNode } from "react";

import { buildRows, dayLabel } from "@/lib/chat/layout";
import { personName } from "@/lib/workspace-name";
import { activeTurnId, type TranscriptState } from "@/lib/chat/transcript";

import { AccessRequestCard } from "./access-request-card";
import { JumpToLatest } from "./jump-to-latest";
import { MessageGroup, StreamingBubble } from "./message-group";
import { PEER_DECIDED_CODE, PEER_WAITING_CODE, PeerApprovalCard, PeerApprovalOutcome } from "./peer-approval-card";
import { BackgroundActivityNotice, ContinueNotice, NoWakeupNotice, RedactedRow, SystemRow, TypingRow } from "./status-rows";
import type { AgentIdent } from "./types";

const s = (v: unknown): string => (typeof v === "string" ? v : "");

/** (FIX-225, D-470) The system line of a follow-up or send-now event: what happened, in words, never a code. */
function systemLine(type: string, p: Record<string, unknown>): string {
  // (C-1102) The canceller is a person `{ id, display_name }`; an address is never shown, `personName` makes a name of it.
  const who = p.by && typeof p.by === "object" ? (p.by as Record<string, unknown>) : null;
  const by = who && typeof who.display_name === "string" ? personName(who.display_name) : "";
  switch (type) {
    case "message.joined":
      return "Follow-up joined the running turn.";
    case "followup.cancelled":
      return by ? `${by} cancelled a queued message.` : "A queued message was cancelled.";
    case "followup.steer_failed":
      return s(p.reason) === "refused"
        ? "A queued message couldn't be steered in (the agent can't take messages right now). It runs after the current turn."
        : "A queued message couldn't be steered in (the agent missed it). It runs after the current turn.";
    case "turn.send_now_failed":
      return s(p.outcome) === "nothing_running" ? "Nothing was running to move to the background." : "This agent can't move a running step to the background.";
    default:
      return "Follow-up sent now.";
  }
}

/** The 2 s highlight of a message the reader jumped to (`accent-soft`; a colour change, so reduced motion is respected). */
const HIGHLIGHT = ["bg-accent-soft", "rounded-card", "outline", "outline-2", "outline-accent"];
const HIGHLIGHT_MS = 2000;
/** Durable events a reader counts as "new messages" while scrolled up. */
const MESSAGE_TYPES = new Set(["turn.queued", "text", "attachment"]);

/**
 * The transcript: a scrolling `role="log"` region of day dividers, sender groups and standalone rows. It is the
 * only part of the thread that scrolls, so the composer below never moves.
 */
export function TranscriptView({
  state,
  wsId,
  base,
  convId,
  agent,
  active,
  empty,
  jump = null,
  onJumpMissing,
  sent = 0,
  after,
}: {
  /** (item 9) Cards under the latest rows: the first successes. */
  after?: ReactNode;
  /** (chat-2) Bumped for every message the person sends: the log goes to the bottom, whatever it was following. */
  sent?: number;
  /** Scroll to (and highlight) the message with this `seq`; a new `n` repeats the request (P5-CHAT-2). */
  jump?: { seq: number; n: number } | null;
  /** The requested message is not in the loaded history. */
  onJumpMissing?: () => void;
  state: TranscriptState;
  wsId: string;
  base: string;
  convId: string;
  agent?: AgentIdent;
  /** A turn is in flight (from the stream or the conversation's `active_turn`). */
  active?: boolean;
  /** Shown in the middle of the log while there is nothing to render. */
  empty?: ReactNode;
}) {
  const now = useMemo(() => new Date(), []);
  const rows = useMemo(() => buildRows(state, now), [state, now]);
  const deltas = Object.values(state.deltas);
  const turn = activeTurnId(state);
  const ref = useRef<HTMLDivElement>(null);
  const stick = useRef(true);

  // Follow new content while the reader is at the bottom; leave them alone when they scrolled up.
  useLayoutEffect(() => {
    const el = ref.current;
    if (el && stick.current) el.scrollTop = el.scrollHeight;
  }, [state]);

  // "Jump to latest": while the reader is more than a screen above the bottom, count the messages that arrived since.
  const [leftAt, setLeftAt] = useState<number | null>(null);

  // (chat-2) After sending, the person's message and the answer are in view, always: follow again, go to the bottom now and
  // once more after the next paint (the message's attachments and the new rows change the height after the first scroll).
  // `leftAt` is reset in the same effect so "Jump to latest" is not shown.
  const sentSeen = useRef(sent);
  useLayoutEffect(() => {
    if (sent === sentSeen.current) return;
    sentSeen.current = sent;
    stick.current = true;
    const el = ref.current;
    if (!el) return;
    el.scrollTop = el.scrollHeight;
    setLeftAt(null);
    const raf = requestAnimationFrame(() => {
      if (ref.current && stick.current) ref.current.scrollTop = ref.current.scrollHeight;
    });
    return () => cancelAnimationFrame(raf);
  }, [sent]);
  const unread = useMemo(
    () => (leftAt === null ? 0 : state.events.filter((e) => (e.seq ?? 0) > leftAt && MESSAGE_TYPES.has(e.type)).length),
    [state.events, leftAt],
  );
  const lastSeqRef = useRef(state.lastSeq);
  useEffect(() => {
    lastSeqRef.current = state.lastSeq;
  }, [state.lastSeq]);

  useEffect(() => {
    if (!jump) return;
    const el = ref.current?.querySelector<HTMLElement>(`[data-seq="${jump.seq}"]`);
    if (!el) {
      onJumpMissing?.();
      return;
    }
    stick.current = false;
    const calm = typeof window.matchMedia === "function" && window.matchMedia("(prefers-reduced-motion: reduce)").matches;
    el.scrollIntoView?.({ block: "center", behavior: calm ? "auto" : "smooth" });
    el.classList.add(...HIGHLIGHT);
    const t = window.setTimeout(() => el.classList.remove(...HIGHLIGHT), HIGHLIGHT_MS);
    return () => {
      window.clearTimeout(t);
      el.classList.remove(...HIGHLIGHT);
    };
    // A new request (`n`) re-runs; the callback identity does not matter.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [jump?.seq, jump?.n]);

  const nothing = rows.length === 0 && deltas.length === 0 && !active;
  return (
    <div className="relative flex min-h-0 flex-1 flex-col">
      <div
        ref={ref}
        role="log"
        aria-live="polite"
        aria-label="Conversation"
        // (chat-2) An image that finishes loading makes the log taller without a scroll event: keep following when following.
        onLoadCapture={() => {
          const el = ref.current;
          if (el && stick.current) el.scrollTop = el.scrollHeight;
        }}
        onScroll={(e) => {
          const el = e.currentTarget;
          const dist = el.scrollHeight - el.scrollTop - el.clientHeight;
          stick.current = dist < 80;
          const away = dist > el.clientHeight;
          setLeftAt((cur) => (away ? (cur ?? lastSeqRef.current) : null));
        }}
        className="flex min-h-0 flex-1 flex-col gap-4 overflow-y-auto overscroll-contain px-3 py-4 md:px-5">
        {nothing && empty ? <div className="my-auto">{empty}</div> : null}
        {rows.map((r) => {
          switch (r.kind) {
            case "day":
              return (
                <div key={r.key} className="flex items-center gap-3 text-xs font-medium text-muted" role="separator" aria-label={r.label}>
                  <span aria-hidden="true" className="h-px flex-1 bg-line" />
                  <span aria-hidden="true">{r.label}</span>
                  <span aria-hidden="true" className="h-px flex-1 bg-line" />
                </div>
              );
            case "group":
              return <MessageGroup key={r.key} row={r} state={state} wsId={wsId} base={base} convId={convId} agent={agent} />;
            case "redacted": {
              const p = r.ev.payload;
              return <RedactedRow key={r.key} by={s(p.redacted_by)} reason={s(p.reason)} date={dayLabel(r.ev.ts, now)} />;
            }
            case "system": {
              const { type, payload: p } = r.ev;
              if (type === "notice") {
                // (chat-4, C-1075) The mark Stop leaves under the partial answer: a muted line between two rules.
                if (s(p.code) === "turn_stopped") {
                  return (
                    <div key={r.key} className="flex items-center gap-3 text-xs text-muted" data-testid="turn-stopped">
                      <span aria-hidden="true" className="h-px flex-1 bg-line" />
                      <span>You stopped this answer</span>
                      <span aria-hidden="true" className="h-px flex-1 bg-line" />
                    </div>
                  );
                }
                if (s(p.code) === "app_turn_cap_reached") {
                  // Only offer Continue until a later durable reset notice (C-27) exists.
                  const reset = state.events.some((e) => (e.seq ?? 0) > (r.ev.seq ?? 0) && e.type === "notice" && s(e.payload.code) === "app_turn_cap_reset");
                  if (!reset) return <ContinueNotice key={r.key} text={s(p.text)} convId={convId} />;
                }
                // (FIX-253, D-490) Background activity: the Stop button only while nothing stands;
                // a durable `background_stop_requested` notice disables it until the acknowledged
                // release, and a later durable `background_stopped` notice mutes the row. A later
                // durable `background_stop_failed` notice (FIX-253 fixes2 B) re-enables Stop so the
                // request can be tried again. (FIX-265) A newer entry replaces the row.
                if (s(p.code) === "background_activity") {
                  // The durable `background_stopped` notice is the visible line; the entry row mutes away.
                  const later = (code: string) => state.events.some((e) => (e.seq ?? 0) > (r.ev.seq ?? 0) && e.type === "notice" && s(e.payload.code) === code);
                  if (later("background_stopped") || later("background_activity")) return null;
                  const requested = later("background_stop_requested") && !later("background_stop_failed");
                  return <BackgroundActivityNotice key={r.key} text={s(p.text)} turnId={s(p.turn_id)} requested={requested} />;
                }
                // (FIX-253, D-490) The self-check notice with its **Remind it in 15 minutes**
                // action; the durable `no_wakeup_set` notice of the same turn disables the button.
                if (s(p.code) === "no_wakeup") {
                  const set = state.events.some(
                    (e) =>
                      (e.seq ?? 0) > (r.ev.seq ?? 0) &&
                      e.type === "notice" &&
                      s(e.payload.code) === "no_wakeup_set" &&
                      s(e.payload.turn_id) === s(p.turn_id),
                  );
                  return <NoWakeupNotice key={r.key} text={s(p.text)} convId={convId} turnId={s(p.turn_id)} set={set} />;
                }
                // (P5-UX-17, C-984) An agent's request for access is a card; its outcome is one line.
                if (s(p.code) === "app_connection_requested" || s(p.code) === "secret_proposed") {
                  return <AccessRequestCard key={r.key} ev={r.ev} events={state.events} wsId={wsId} agent={agent} />;
                }
                if (s(p.code) === "app_access_given" || s(p.code) === "app_access_request_dismissed") {
                  return (
                    <SystemRow key={r.key}>
                      <span data-testid="access-request-outcome">{s(p.text)}</span>
                    </SystemRow>
                  );
                }
                // (FIX-252) A delegated task's approval at the origin: the wait is a card whose
                // buttons follow the reader's rights at the approval's workspace; its outcome is
                // the settled line.
                if (s(p.code) === PEER_WAITING_CODE) {
                  return <PeerApprovalCard key={r.key} ev={r.ev} events={state.events} />;
                }
                if (s(p.code) === PEER_DECIDED_CODE) {
                  return <PeerApprovalOutcome key={r.key} ev={r.ev} />;
                }
                return <SystemRow key={r.key}>{s(p.text)}</SystemRow>;
              }
              if (type === "correction") {
                return (
                  <SystemRow key={r.key}>
                    {p.action === "retract" ? "A previous message was retracted." : `A previous message was corrected: ${s(p.text)}`}
                  </SystemRow>
                );
              }
              return <SystemRow key={r.key}>{systemLine(type, p)}</SystemRow>;
            }
          }
        })}
        {deltas.map((d) => (
          <StreamingBubble key={d.messageId} text={d.text} agent={agent} />
        ))}
        {active ? <TypingRow agent={agent} line={turn ? state.activity[turn] : undefined} /> : null}
        {after}
      </div>
      {leftAt !== null ? (
        <JumpToLatest
          count={unread}
          onClick={() => {
            const el = ref.current;
            if (!el) return;
            stick.current = true;
            el.scrollTo?.({ top: el.scrollHeight });
            setLeftAt(null);
          }}
        />
      ) : null}
    </div>
  );
}
