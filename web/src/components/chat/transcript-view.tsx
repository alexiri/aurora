"use client";

import { useLayoutEffect, useMemo, useRef, type ReactNode } from "react";

import { buildRows, dayLabel } from "@/lib/chat/layout";
import { activeTurnId, type TranscriptState } from "@/lib/chat/transcript";

import { MessageGroup, StreamingBubble } from "./message-group";
import { ContinueNotice, RedactedRow, SystemRow, TypingRow } from "./status-rows";
import type { AgentIdent } from "./types";

const s = (v: unknown): string => (typeof v === "string" ? v : "");

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
}: {
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

  const nothing = rows.length === 0 && deltas.length === 0 && !active;
  return (
    <div
      ref={ref}
      role="log"
      aria-live="polite"
      aria-label="Conversation"
      onScroll={(e) => {
        const el = e.currentTarget;
        stick.current = el.scrollHeight - el.scrollTop - el.clientHeight < 80;
      }}
      className="flex min-h-0 flex-1 flex-col gap-4 overflow-y-auto overscroll-contain px-3 py-4 md:px-5"
    >
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
              return <SystemRow key={r.key}>{p.action === "retract" ? "A previous message was retracted." : `A previous message was corrected: ${s(p.text)}`}</SystemRow>;
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
  );
}
