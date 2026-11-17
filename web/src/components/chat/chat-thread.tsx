"use client";

import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import { ConversationActions } from "@/components/agents/conversation-actions";
import { Avatar } from "@/components/agent-avatar";
import { ErrorState, Loading } from "@/components/states";
import { Button } from "@/components/ui/button";
import { DropZone } from "@/components/ui/drop-zone";
import { Notice } from "@/components/ui/notice";
import { ApiError } from "@/lib/api/client";
import { chatApi, type Conversation } from "@/lib/api/conversations";
import { conversationTitle, kindLabel } from "@/lib/chat/layout";
import { buildPermalink, permalinkSeq, withoutMessageParam } from "@/lib/chat/permalink";
import { activeTurnId } from "@/lib/chat/transcript";
import { useTranscript } from "@/lib/chat/use-transcript";

import { ChatActionsContext, type ChatActions, type ReplyTarget } from "./chat-actions";
import { AccountsBanner } from "@/components/accounts/attention";
import { Composer } from "./composer";
import { ThreadHeader } from "./thread-header";
import { TranscriptView } from "./transcript-view";
import type { AgentIdent } from "./types";

const DATE = new Intl.DateTimeFormat("en-GB", { day: "numeric", month: "short", year: "numeric" });

/** The thread: header, transcript and the pinned composer (or the read-only notice on another member's direct chat). */
export function ChatThread({
  agentId,
  wsId,
  base,
  convId,
  direct,
  conversation = null,
  ownerView = null,
  ownDirect = false,
  archived = false,
  retentionDays = null,
  onGone,
  onBack,
  onTurn,
  agent,
}: {
  agentId: string;
  wsId: string;
  base: string;
  convId: string;
  agent?: AgentIdent;
  /** The caller's own direct conversation, when this is it. */
  direct: Conversation | null;
  /** The listed conversation this thread shows (kind, title, archived date). */
  conversation?: Conversation | null;
  /** Display name of the person whose direct conversation an owner is reading (read-only). */
  ownerView?: string | null;
  ownDirect?: boolean;
  /** The conversation is archived (RFE-010): offer Unarchive instead of Archive. */
  archived?: boolean;
  /** The org's conversation retention window in days, when one is set. */
  retentionDays?: number | null;
  onGone?: () => void;
  /** Phones: back to the list. */
  onBack?: () => void;
  /** (P5-AGENT-2) Reports whether a turn runs and its latest activity line, for the status rail ("Doing now"). */
  onTurn?: (turn: { active: boolean; line: string | null }) => void;
}) {
  const { state, loading, error, live } = useTranscript(convId);
  const [actionErr, setActionErr] = useState<string | null>(null);
  const [stopErr, setStopErr] = useState<string | null>(null);
  // (P5-CHAT-2) Reply target, jump requests and the "not available" notice for a permalink or quote that points nowhere.
  const [reply, setReply] = useState<ReplyTarget | null>(null);
  // Files dropped anywhere on the thread pane go to the composer's uploader (P5-CHAT-3).
  const attachRef = useRef<((files: File[]) => void) | null>(null);
  // A permalink (`?c=&m=`) is read at mount; the transcript performs the jump once its history is shown.
  const [jump, setJump] = useState<{ seq: number; n: number } | null>(() => {
    const seq = typeof window === "undefined" ? null : permalinkSeq(window.location.search, convId);
    return seq === null ? null : { seq, n: 1 };
  });
  const [missing, setMissing] = useState(false);
  const actions = useMemo<ChatActions>(
    () => ({
      reply: setReply,
      jump: (seq) => {
        setMissing(false);
        setJump((j) => ({ seq, n: (j?.n ?? 0) + 1 }));
      },
      link: (seq) => buildPermalink(window.location.href, convId, seq),
    }),
    [convId],
  );
  // The jump is one-shot: drop `m` from the address once the history has loaded, so a reload does not repeat it.
  const ready = !loading && !error;
  useEffect(() => {
    if (ready && permalinkSeq(window.location.search, convId) !== null) {
      window.history.replaceState(window.history.state, "", withoutMessageParam(window.location.href));
    }
  }, [ready, convId]);
  const onMissing = useCallback(() => setMissing(true), []);
  const active = activeTurnId(state, conversation ?? direct);
  const line = active ? (state.activity[active] ?? null) : null;
  useEffect(() => {
    onTurn?.({ active: !!active, line });
  }, [active, line, onTurn]);
  // A conversation switch unmounts this thread: the rail must not keep its last turn.
  useEffect(() => () => onTurn?.({ active: false, line: null }), [onTurn]);
  const agentName = agent?.name ?? "the agent";
  const readOnly = ownerView !== null;
  const isDirect = ownDirect || !!direct || readOnly || conversation?.kind === "direct";
  const empty = !loading && !error && state.events.length === 0 && Object.keys(state.deltas).length === 0;

  const title = conversation ? conversationTitle(conversation) : readOnly ? ownerView || "Direct chat" : "Direct chat";
  const visibility = isDirect
    ? readOnly
      ? `Only ${ownerView || "the member"} and workspace owners can see this conversation`
      : "Only you and workspace owners can see this conversation"
    : undefined;
  const segments = [
    ...(isDirect || !conversation ? [] : [kindLabel(conversation.kind)]),
    ...(conversation?.archived_at ? [`Archived ${DATE.format(new Date(conversation.archived_at))}`] : []),
    ...(retentionDays ? [`Deleted after ${retentionDays} days without activity`] : []),
  ];

  return (
    <ChatActionsContext.Provider value={actions}>
      <DropZone className="flex min-h-0 flex-1 flex-col" disabled={readOnly} onFiles={(files) => attachRef.current?.(files)}>
      <section aria-label="Chat" className="flex min-h-0 flex-1 flex-col">
        <ThreadHeader
          title={title}
          visibility={visibility}
          segments={segments}
          live={loading || error ? null : live}
          onBack={onBack}
          actions={onGone ? <ConversationActions agentId={agentId} convId={convId} archived={archived} onGone={onGone} onError={setActionErr} /> : null}
        />
        {actionErr ? (
          <Notice tone="danger" role="alert" className="m-3 shrink-0">
            {actionErr}
          </Notice>
        ) : null}
        {loading ? (
          <div className="flex-1 p-4">
            <Loading />
          </div>
        ) : error ? (
          <div className="flex-1 p-4">
            <ErrorState error={error} />
          </div>
        ) : (
          <TranscriptView
            jump={jump}
            onJumpMissing={onMissing}
            state={state}
            wsId={wsId}
            base={base}
            convId={convId}
            agent={agent}
            active={!!active}
            empty={
              empty ? (
                <div className="mx-auto flex max-w-sm flex-col items-center gap-2 text-center">
                  <Avatar name={agent?.name ?? "Agent"} url={agent?.avatarUrl} tintKey={agent?.id} size={72} />
                  <h3 className="font-display text-lg font-semibold text-ink">Say hello to {agent?.name ?? "the agent"}</h3>
                  {agent?.handle ? <p className="text-sm text-ink-2">@{agent.handle}</p> : null}
                </div>
              ) : undefined
            }
          />
        )}
        {missing ? (
          <Notice role="status" className="mx-3 mb-2 shrink-0 md:mx-5">
            Message not available.
          </Notice>
        ) : null}
        {readOnly ? (
          <div className="shrink-0 space-y-2 border-t border-line p-3 md:px-5">
            {stopErr ? (
              <p role="alert" className="text-sm text-danger-ink">
                {stopErr}
              </p>
            ) : null}
            <Notice role="note" className="flex flex-wrap items-center gap-2">
              <p className="min-w-0 flex-1">
                Read-only view. This is {ownerView || "another member"}&apos;s direct conversation. You can read it because you own this workspace; your access
                is recorded in the audit log. You can stop its turns but cannot write in it.
              </p>
              {active ? (
                <Button
                  type="button"
                  variant="outline"
                  size="sm"
                  onClick={() => {
                    setStopErr(null);
                    chatApi.abort(active).catch((e: unknown) => setStopErr(e instanceof ApiError ? e.message : "Could not stop the turn."));
                  }}>
                  Stop
                </Button>
              ) : null}
            </Notice>
          </div>
        ) : (
          <>
            <AccountsBanner workspaceId={wsId} agent={{ id: agentId, name: agentName }} base={base} />
          <Composer
            agentId={agentId}
            convId={convId}
            base={base}
            agentName={agentName}
            activeTurn={active}
            state={state}
            autoFocus={empty}
            reply={reply}
            onClearReply={() => setReply(null)}
            onRestoreReply={setReply}
            attachRef={attachRef}
          />
          </>
        )}
      </section>
      </DropZone>
    </ChatActionsContext.Provider>
  );
}
