"use client";

import { useEffect, useLayoutEffect, useMemo, useRef, useState, type ReactNode } from "react";

import { buildRows, dayLabel } from "@/lib/chat/layout";
import { activeTurnId, type TranscriptState } from "@/lib/chat/transcript";

import { JumpToLatest } from "./jump-to-latest";
import { MessageGroup, StreamingBubble } from "./message-group";
import { ContinueNotice, RedactedRow, SystemRow, TypingRow } from "./status-rows";
import type { AgentIdent } from "./types";

const s = (v: unknown): string => (typeof v === "string" ? v : "");

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
}: {
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
                if (s(p.code) === "app_turn_cap_reached") {
                  // Only offer Continue until a later durable reset notice (C-27) exists.
                  const reset = state.events.some((e) => (e.seq ?? 0) > (r.ev.seq ?? 0) && e.type === "notice" && s(e.payload.code) === "app_turn_cap_reset");
                  if (!reset) return <ContinueNotice key={r.key} text={s(p.text)} convId={convId} />;
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
              return <SystemRow key={r.key}>{type === "message.joined" ? "Follow-up joined the running turn." : "Follow-up sent now."}</SystemRow>;
            }
          }
        })}
        {deltas.map((d) => (
          <StreamingBubble key={d.messageId} text={d.text} agent={agent} />
        ))}
        {active ? <TypingRow agent={agent} line={turn ? state.activity[turn] : undefined} /> : null}
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
