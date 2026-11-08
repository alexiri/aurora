"use client";

import { useEffect, useState, useSyncExternalStore } from "react";

import { Avatar } from "@/components/agent-avatar";
import { Markdown } from "@/components/apps/markdown";
import { RedactDialog } from "@/components/agents/conversation-actions";
import { Badge } from "@/components/ui/badge";
import { CheckIcon, CopyIcon, MoreIcon } from "@/components/ui/icons";
import { Menu, MenuContent, MenuItem, MenuTrigger } from "@/components/ui/menu";
import type { TranscriptEvent } from "@/lib/api/conversations";
import { clockTime, type Block, type Row } from "@/lib/chat/layout";
import type { TranscriptState } from "@/lib/chat/transcript";
import { cn } from "@/lib/utils";

import { ActionApprovalNote, ApprovalCard } from "./approval-card";
import { AttachmentCard } from "./attachments";
import { ErrorBlock } from "./status-rows";
import { ToolCallGroup } from "./tool-call-group";
import type { AgentIdent } from "./types";

const s = (v: unknown): string => (typeof v === "string" ? v : "");

const noSubscribe = () => () => undefined;

const iconButton =
  "inline-flex size-11 items-center justify-center rounded-control text-ink-2 hover:bg-subtle hover:text-ink md:size-8";

/**
 * The small toolbar at a bubble's top corner: Copy message and a "More message actions" menu with Redact…
 * It shows on hover, on focus within, and when the message is tapped (`shown`). It stays in the tab order
 * while hidden, so keyboard users reach it by tabbing into the message.
 */
function MessageToolbar({ convId, seq, text, shown, side }: { convId: string; seq: number; text: string; shown: boolean; side: "person" | "agent" }) {
  const [redact, setRedact] = useState(false);
  const [menuOpen, setMenuOpen] = useState(false);
  const [copied, setCopied] = useState(false);
  // The clipboard API is missing on insecure origins: hide the item rather than ship a broken one.
  const canCopy = useSyncExternalStore(
    noSubscribe,
    () => typeof navigator !== "undefined" && !!navigator.clipboard?.writeText,
    () => false,
  );
  useEffect(() => {
    if (!copied) return;
    const t = window.setTimeout(() => setCopied(false), 1500);
    return () => window.clearTimeout(t);
  }, [copied]);

  return (
    <>
      <div
        role="toolbar"
        aria-label="Message actions"
        className={cn(
          "absolute -top-4 z-10 flex rounded-control border border-line bg-surface text-ink shadow-card dark:bg-canvas",
          side === "person" ? "right-2" : "left-2",
          // No fade: a focused control must never be (even briefly) invisible. The menu keeps it shown while open,
          // because focus then sits in the portalled menu.
          shown || menuOpen
            ? "opacity-100"
            : "pointer-events-none opacity-0 focus-within:pointer-events-auto focus-within:opacity-100 group-focus-within/msg:pointer-events-auto group-focus-within/msg:opacity-100 group-hover/msg:pointer-events-auto group-hover/msg:opacity-100",
        )}
      >
        {canCopy ? (
          <button
            type="button"
            className={iconButton}
            aria-label={copied ? "Copied" : "Copy message"}
            onClick={() => void navigator.clipboard.writeText(text).then(() => setCopied(true), () => undefined)}
          >
            {copied ? <CheckIcon aria-hidden="true" /> : <CopyIcon aria-hidden="true" />}
          </button>
        ) : null}
        <Menu modal={false} onOpenChange={setMenuOpen}>
          <MenuTrigger asChild>
            <button type="button" className={iconButton} aria-label="More message actions">
              <MoreIcon aria-hidden="true" />
            </button>
          </MenuTrigger>
          <MenuContent align="end">
            <MenuItem onSelect={() => setRedact(true)}>Redact…</MenuItem>
          </MenuContent>
        </Menu>
      </div>
      <RedactDialog convId={convId} seq={seq} open={redact} onOpenChange={setRedact} />
    </>
  );
}

function Bubble({
  ev,
  state,
  convId,
  side,
}: {
  ev: TranscriptEvent;
  state: TranscriptState;
  convId: string;
  side: "person" | "agent";
}) {
  const [shown, setShown] = useState(false);
  const p = ev.payload;
  const text = s(p.text);
  const attachments = side === "person" && Array.isArray(p.attachments) ? (p.attachments as unknown[]).map(s).filter(Boolean) : [];
  return (
    <div className={cn("group/msg relative flex max-w-full", side === "person" ? "justify-end" : "justify-start")}>
      <div
        onClick={() => setShown((v) => !v)}
        className={cn(
          "min-w-0 space-y-2 break-words px-3.5 py-2.5 text-sm",
          side === "person"
            ? "max-w-[min(78%,620px)] rounded-card rounded-tr-[4px] bg-bubble text-bubble-ink"
            : "max-w-[min(88%,720px)] rounded-card rounded-tl-[4px] bg-subtle text-ink",
          "[&_code]:rounded [&_code]:border [&_code]:border-line [&_code]:bg-surface [&_code]:px-1 [&_code]:text-ink dark:[&_code]:bg-canvas",
          "[&_pre]:border [&_pre]:border-line [&_pre]:bg-surface dark:[&_pre]:bg-canvas [&_pre_code]:border-0 [&_pre_code]:px-0",
        )}
      >
        {side === "person" ? <p className="whitespace-pre-wrap">{text}</p> : <Markdown source={text} />}
        {attachments.map((id) => (
          <AttachmentCard key={id} id={id} purged={state.purged?.[id]} />
        ))}
      </div>
      {ev.seq ? <MessageToolbar convId={convId} seq={ev.seq} text={text} shown={shown} side={side} /> : null}
    </div>
  );
}

function BlockView({
  block,
  state,
  wsId,
  base,
  convId,
  side,
}: {
  block: Block;
  state: TranscriptState;
  wsId: string;
  base: string;
  convId: string;
  side: "person" | "agent";
}) {
  switch (block.kind) {
    case "bubble":
      return <Bubble ev={block.ev} state={state} convId={convId} side={side} />;
    case "tools":
      return <ToolCallGroup calls={block.calls} running={block.running} />;
    case "approval":
      return block.ev.type === "approval.pending" ? (
        <ApprovalCard ev={block.ev} state={state} wsId={wsId} />
      ) : (
        <ActionApprovalNote summary={s(block.ev.payload.summary)} wsId={wsId} />
      );
    case "attachment": {
      const p = block.ev.payload;
      const id = s(p.attachment_id);
      return (
        <AttachmentCard
          id={id}
          purged={state.purged?.[id]}
          filename={s(p.filename)}
          contentType={s(p.content_type)}
          size={typeof p.size === "number" ? p.size : typeof p.size_bytes === "number" ? p.size_bytes : undefined}
        />
      );
    }
    case "error": {
      const p = block.ev.payload;
      return block.ev.type === "error" ? (
        <ErrorBlock code={s(p.code)} message={s(p.message)} base={base} />
      ) : (
        <ErrorBlock code={s(p.error_code)} base={base} />
      );
    }
  }
}

/** One sender group: the meta line (name, time, door chip) once, then that sender's blocks. */
export function MessageGroup({
  row,
  state,
  wsId,
  base,
  convId,
  agent,
}: {
  row: Extract<Row, { kind: "group" }>;
  state: TranscriptState;
  wsId: string;
  base: string;
  convId: string;
  agent?: AgentIdent;
}) {
  const person = row.side === "person";
  const name = person ? row.name : (agent?.name ?? "Agent");
  const meta = (
    <p className={cn("flex flex-wrap items-center gap-x-2 gap-y-0.5 text-xs text-muted", person && "justify-end")}>
      <span className="text-xs font-semibold text-ink">{name}</span>
      <time dateTime={row.ts}>{clockTime(row.ts)}</time>
      {row.door ? <Badge>{row.door}</Badge> : null}
    </p>
  );
  const blocks = row.blocks.map((b, i) => (
    <BlockView key={b.kind === "tools" ? b.key : `${b.kind}-${"ev" in b ? b.ev.seq : i}`} block={b} state={state} wsId={wsId} base={base} convId={convId} side={row.side} />
  ));
  if (person) {
    return (
      <div className="flex flex-col items-end gap-1.5">
        {meta}
        {blocks}
      </div>
    );
  }
  return (
    <div className="flex items-start gap-2">
      <Avatar name={agent?.name ?? "Agent"} url={agent?.avatarUrl} tintKey={agent?.id} size={32} />
      <div className="min-w-0 flex-1 space-y-1.5">
        {meta}
        {blocks}
      </div>
    </div>
  );
}

/** A streaming (not yet durable) answer: the agent bubble without the toolbar. */
export function StreamingBubble({ text, agent }: { text: string; agent?: AgentIdent }) {
  return (
    <div className="flex items-start gap-2">
      <Avatar name={agent?.name ?? "Agent"} url={agent?.avatarUrl} tintKey={agent?.id} size={32} />
      <div className="min-w-0 max-w-[min(88%,720px)] rounded-card rounded-tl-[4px] bg-subtle px-3.5 py-2.5 text-sm text-ink">
        <Markdown source={text} />
      </div>
    </div>
  );
}
