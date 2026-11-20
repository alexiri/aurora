"use client";

import { useEffect, useRef, useState, useSyncExternalStore, type ReactNode } from "react";

import { Avatar } from "@/components/agent-avatar";
import { Altitude } from "@/components/ui/altitude";
import { Markdown } from "@/components/ui/markdown";
import { RedactDialog } from "@/components/agents/conversation-actions";
import { Badge } from "@/components/ui/badge";
import { CheckIcon, CopyIcon, MoreIcon, ReplyIcon } from "@/components/ui/icons";
import { Menu, MenuContent, MenuItem, MenuTrigger } from "@/components/ui/menu";
import { ApiError } from "@/lib/api/client";
import type { TranscriptEvent } from "@/lib/api/conversations";
import { chatApi } from "@/lib/api/conversations";
import { useWorkspace } from "@/lib/api/queries";
import { newIdempotencyKey } from "@/lib/chat/idempotency";
import { clockTime, type Block, type Row } from "@/lib/chat/layout";
import type { TranscriptState } from "@/lib/chat/transcript";
import { cn } from "@/lib/utils";

import { ActionApprovalNote, ApprovalCard } from "./approval-card";
import { AttachmentCard } from "./attachments";
import { excerptOf, useChatActions } from "./chat-actions";
import { QuoteStrip, quoteOf } from "./quote-strip";
import { ErrorBlock, RetryStart, isRetryableCode } from "./status-rows";
import { ToolCallGroup } from "./tool-call-group";
import type { AgentIdent } from "./types";

const s = (v: unknown): string => (typeof v === "string" ? v : "");

const noSubscribe = () => () => undefined;

const iconButton = "inline-flex size-11 items-center justify-center rounded-control text-ink-2 hover:bg-subtle hover:text-ink md:size-8";

/**
 * The small toolbar at a bubble's top corner: Copy message and a "More message actions" menu with Redact…
 * It shows on hover, on focus within, and when the message is tapped (`shown`). It stays in the tab order
 * while hidden, so keyboard users reach it by tabbing into the message.
 */
function MessageToolbar({
  convId,
  seq,
  text,
  shown,
  side,
  author,
}: {
  convId: string;
  seq: number;
  text: string;
  shown: boolean;
  side: "person" | "agent";
  /** Who wrote it: named in the reply bar. */
  author: string;
}) {
  const actions = useChatActions();
  const [redact, setRedact] = useState(false);
  const [menuOpen, setMenuOpen] = useState(false);
  const [copied, setCopied] = useState(false);
  const [linked, setLinked] = useState(false);
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
  useEffect(() => {
    if (!linked) return;
    const t = window.setTimeout(() => setLinked(false), 1500);
    return () => window.clearTimeout(t);
  }, [linked]);

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
        )}>
        <button type="button" className={iconButton} aria-label="Reply" onClick={() => actions.reply({ seq, author, excerpt: excerptOf(text) })}>
          <ReplyIcon aria-hidden="true" />
        </button>
        {canCopy ? (
          <button
            type="button"
            className={iconButton}
            aria-label={copied ? "Copied" : "Copy message"}
            onClick={() =>
              void navigator.clipboard.writeText(text).then(
                () => setCopied(true),
                () => undefined,
              )
            }>
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
            {canCopy ? (
              <MenuItem
                onSelect={() =>
                  void navigator.clipboard.writeText(actions.link(seq)).then(
                    () => setLinked(true),
                    () => undefined,
                  )
                }>
                {linked ? "Link copied" : "Copy link"}
              </MenuItem>
            ) : null}
            <Altitude id="chat.redact" level="expert">
              <MenuItem onSelect={() => setRedact(true)}>Remove this message&apos;s content…</MenuItem>
            </Altitude>
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
  author,
}: {
  ev: TranscriptEvent;
  state: TranscriptState;
  convId: string;
  side: "person" | "agent";
  author: string;
}) {
  const [shown, setShown] = useState(false);
  const actions = useChatActions();
  const p = ev.payload;
  const text = s(p.text);
  const quote = quoteOf(p);
  const attachments = side === "person" && Array.isArray(p.attachments) ? (p.attachments as unknown[]).map(s).filter(Boolean) : [];
  return (
    // `w-full`: the row spans the column, so the bubble's percentage max-width is measured against the chat
    // column. In the person column (`items-end`) a shrink-to-fit row made 80 % mean "80 % of the text's own
    // width", wrapping "hi" into "h / i".
    <div
      data-seq={ev.seq || undefined}
      onKeyDown={(e) => {
        // R on a focused message (or one of its controls) replies to it.
        if (
          (e.key === "r" || e.key === "R") &&
          !e.ctrlKey &&
          !e.metaKey &&
          !e.altKey &&
          ev.seq &&
          !(e.target instanceof HTMLTextAreaElement || e.target instanceof HTMLInputElement)
        ) {
          e.preventDefault();
          actions.reply({ seq: ev.seq, author, excerpt: excerptOf(text) });
        }
      }}
      className={cn("group/msg relative flex w-full", side === "person" ? "justify-end" : "justify-start")}>
      <div
        onClick={() => setShown((v) => !v)}
        className={cn(
          "min-w-0 space-y-2 break-words px-3.5 py-2.5 text-sm",
          side === "person"
            ? "max-w-bubble rounded-card rounded-tr-tail bg-bubble text-bubble-ink"
            : "max-w-bubble rounded-card rounded-tl-tail bg-subtle text-ink",
        )}>
        {quote ? <QuoteStrip quote={quote} onJump={actions.jump} side={side} /> : null}
        {text.trim() ? <Markdown source={text} breaks={side === "person"} /> : null}
        {attachments.map((id) => (
          <AttachmentCard key={id} id={id} purged={state.purged?.[id]} />
        ))}
      </div>
      {ev.seq ? <MessageToolbar convId={convId} seq={ev.seq} text={text} shown={shown} side={side} author={author} /> : null}
    </div>
  );
}

/** An agent's file with the message toolbar (Reply, Copy link, Redact…) and the `data-seq` jump anchor. */
function FileMessage({
  seq,
  convId,
  name,
  caption,
  author,
  children,
}: {
  seq: number;
  convId: string;
  name: string;
  caption: string;
  author: string;
  children: ReactNode;
}) {
  const [shown, setShown] = useState(false);
  const actions = useChatActions();
  const text = [name, caption].filter(Boolean).join("\n");
  return (
    <div
      data-seq={seq}
      onClick={() => setShown((v) => !v)}
      onKeyDown={(e) => {
        if ((e.key === "r" || e.key === "R") && !e.ctrlKey && !e.metaKey && !e.altKey) {
          e.preventDefault();
          actions.reply({ seq, author, excerpt: excerptOf(text) });
        }
      }}
      className="group/msg relative flex w-full justify-start">
      <div className="min-w-0 max-w-bubble">{children}</div>
      <MessageToolbar convId={convId} seq={seq} text={text} shown={shown} side="agent" author={author} />
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
  agentId,
  frozen,
  author,
}: {
  author: string;
  block: Block;
  state: TranscriptState;
  wsId: string;
  base: string;
  convId: string;
  side: "person" | "agent";
  /** The agent of the chat, when known (the Chat tab passes it): enables **Retry now** (C-615). */
  agentId?: string;
  /** A frozen agent has no **Retry now** (C-615). */
  frozen?: boolean;
}) {
  switch (block.kind) {
    case "bubble":
      return <Bubble ev={block.ev} state={state} convId={convId} side={side} author={author} />;
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
      const card = (
        <AttachmentCard
          id={id}
          purged={state.purged?.[id]}
          filename={s(p.filename)}
          contentType={s(p.content_type)}
          size={typeof p.size === "number" ? p.size : typeof p.size_bytes === "number" ? p.size_bytes : undefined}
        />
      );
      // An agent's file can be replied to and linked like a message (C-725).
      return block.ev.seq && side === "agent" ? (
        <FileMessage seq={block.ev.seq} convId={convId} name={s(p.filename)} caption={s(p.caption)} author={author}>
          {card}
        </FileMessage>
      ) : (
        card
      );
    }
    case "error": {
      const p = block.ev.payload;
      const turnId = s(p.turn_id);
      const retryProps = agentId && turnId && !frozen ? { agentId, turnId, convId, state } : null;
      if (block.ev.type === "error") {
        const code = s(p.code);
        return retryProps ? (
          <TurnErrorWithRetry code={code} message={s(p.message)} base={base} wsId={wsId} {...retryProps} />
        ) : (
          <TurnError code={code} message={s(p.message)} base={base} wsId={wsId} agentId={agentId} />
        );
      }
      const code = s(p.error_code);
      return retryProps ? <TurnErrorWithRetry code={code} base={base} wsId={wsId} {...retryProps} /> : <TurnError code={code} base={base} wsId={wsId} agentId={agentId} />;
    }
  }
}

/** The error block, with the owner-only links ("Agent access") decided from the caller's role in the workspace. */
function TurnError({ code, message, base, wsId, agentId }: { code: string; message?: string; base: string; wsId: string; agentId?: string }) {
  const ws = useWorkspace(wsId);
  return <ErrorBlock code={code} message={message} base={base} agentId={agentId} canOwn={ws.data?.my_role === "owner"} />;
}

/** The error block with **Retry now** (FIX-029, C-615): the chat's start-failure codes, agent-side. */
function TurnErrorWithRetry({
  code,
  message,
  base,
  wsId,
  agentId,
  turnId,
  convId,
  state,
}: {
  code: string;
  message?: string;
  base: string;
  wsId: string;
  agentId: string;
  /** The failed turn: its text (from its `turn.queued` event) is re-posted after a 202 (C-615). */
  turnId: string;
  convId: string;
  state: TranscriptState;
}) {
  const ws = useWorkspace(wsId);
  // One key per failed turn (RFE-018): a second click, or a retry after a lost response, never opens a second turn.
  const keyRef = useRef<string | null>(null);
  const [repostErr, setRepostErr] = useState<string | null>(null);
  // (C-615) Only the start-failure codes the retry endpoint accepts get the button (C-585, C-610).
  if (!isRetryableCode(code)) {
    return <ErrorBlock code={code} message={message} base={base} agentId={agentId} canOwn={ws.data?.my_role === "owner"} />;
  }
  return (
    <ErrorBlock code={code} message={message} base={base} agentId={agentId} canOwn={ws.data?.my_role === "owner"}>
      <RetryStart
        agentId={agentId}
        // (C-615) The failed turn stays failed; the re-posted text opens the next turn.
        onRetried={() => {
          const queued = state.events.find((e) => e.type === "turn.queued" && e.payload.turn_id === turnId);
          const text = typeof queued?.payload.text === "string" ? queued.payload.text : "";
          if (!text.trim()) return;
          keyRef.current ??= newIdempotencyKey();
          setRepostErr(null);
          chatApi.send(convId, text, [], keyRef.current).then(
            () => {
              keyRef.current = null;
            },
            (e: unknown) => {
              if (e instanceof ApiError && e.code === "idempotency_key_reused") keyRef.current = null;
              setRepostErr(e instanceof ApiError ? e.message : "The agent's computer restarted, but your message was not re-sent. Send it again.");
            },
          );
        }}
      />
      {repostErr ? (
        <span role="alert" className="mt-1 block text-xs">
          {repostErr}
        </span>
      ) : null}
    </ErrorBlock>
  );
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
    <BlockView
      key={b.kind === "tools" ? b.key : `${b.kind}-${"ev" in b ? b.ev.seq : i}`}
      block={b}
      state={state}
      wsId={wsId}
      base={base}
      convId={convId}
      side={row.side}
      agentId={agent?.id}
      frozen={agent?.frozen}
      author={name}
    />
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
      <div className="min-w-0 max-w-bubble rounded-card rounded-tl-tail bg-subtle px-3.5 py-2.5 text-sm text-ink">
        <Markdown source={text} streaming />
      </div>
    </div>
  );
}
