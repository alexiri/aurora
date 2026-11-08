"use client";

import { useState } from "react";

import { ConversationActions } from "@/components/agents/conversation-actions";
import { Avatar } from "@/components/agent-avatar";
import { ErrorState, Loading } from "@/components/states";
import { Button } from "@/components/ui/button";
import { Notice } from "@/components/ui/notice";
import { ApiError } from "@/lib/api/client";
import { chatApi, type Conversation } from "@/lib/api/conversations";
import { conversationTitle, kindLabel } from "@/lib/chat/layout";
import { activeTurnId } from "@/lib/chat/transcript";
import { useTranscript } from "@/lib/chat/use-transcript";

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
}) {
  const { state, loading, error, live } = useTranscript(convId);
  const [actionErr, setActionErr] = useState<string | null>(null);
  const [stopErr, setStopErr] = useState<string | null>(null);
  const active = activeTurnId(state, conversation ?? direct);
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
    <section aria-label="Chat" className="flex min-h-0 flex-1 flex-col">
      <ThreadHeader
        title={title}
        visibility={visibility}
        segments={segments}
        live={loading || error ? null : live}
        onBack={onBack}
        actions={
          onGone ? <ConversationActions agentId={agentId} convId={convId} archived={archived} onGone={onGone} onError={setActionErr} /> : null
        }
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
                <h3 className="font-display text-lg font-semibold text-ink">Start a conversation with {agent?.name ?? "the agent"}</h3>
                {agent?.brief ? <p className="text-sm text-ink-2">{agent.brief}</p> : null}
              </div>
            ) : undefined
          }
        />
      )}
      {readOnly ? (
        <div className="shrink-0 space-y-2 border-t border-line p-3 md:px-5">
          {stopErr ? (
            <p role="alert" className="text-sm text-danger-ink">
              {stopErr}
            </p>
          ) : null}
          <Notice role="note" className="flex flex-wrap items-center gap-2">
            <p className="min-w-0 flex-1">
              Read-only view. This is {ownerView || "another member"}&apos;s direct conversation. You can read it because you own this workspace; your access is
              recorded in the audit log. You can stop its turns but cannot write in it.
            </p>
            {active ? (
              <Button
                type="button"
                variant="outline"
                size="sm"
                onClick={() => {
                  setStopErr(null);
                  chatApi.abort(active).catch((e: unknown) => setStopErr(e instanceof ApiError ? e.message : "Could not stop the turn."));
                }}
              >
                Stop
              </Button>
            ) : null}
          </Notice>
        </div>
      ) : (
        <Composer agentId={agentId} convId={convId} base={base} agentName={agentName} activeTurn={active} state={state} autoFocus={empty} />
      )}
    </section>
  );
}
