"use client";

import { useQueryClient } from "@tanstack/react-query";
import { useCallback, useEffect, useMemo, useState, useSyncExternalStore } from "react";

import { ChatPanel } from "@/components/chat/chat-panel";
import { ChatThread } from "@/components/chat/chat-thread";
import { ConversationList } from "@/components/chat/conversation-list";
import type { AgentIdent } from "@/components/chat/types";
import { ErrorState, Loading } from "@/components/states";
import { ChevronRightIcon } from "@/components/ui/icons";
import { useApprovalsStream, useCanApprove } from "@/lib/api/approvals";
import { useConversationById, useConversations, useDirectConversation, type Conversation } from "@/lib/api/conversations";
import { useRetention } from "@/lib/api/org";
import { conversationTitle, filterConversations, groupConversations, sortByActivity, type ListGroups } from "@/lib/chat/layout";

import type { AgentPageId } from "./sections";
import { useRailExpanded } from "./status-rail/rail-state";
import { StatusRail, StatusSheet, type RailContext } from "./status-rail/status-rail";
import { useAgentStatus, type OpenTurn } from "./status-rail/status-data";

/** Keeps the pending approvals live (the stream refetches the agent-scoped lists); approvers only. */
function ApprovalsLive({ wsId }: { wsId: string }) {
  useApprovalsStream(wsId);
  return null;
}

export type { AgentIdent } from "@/components/chat/types";
/** The thread (header, transcript, composer) for one conversation. */
export { ChatThread as ChatPane } from "@/components/chat/chat-thread";

/** The selected conversation lives in the URL (`?c=<id>`; no param = the caller's direct chat), so back and forward work and a deep link opens it. */
const PARAM = "c";
const URL_EVENT = "aurora:chat-url";

function subscribe(cb: () => void) {
  window.addEventListener("popstate", cb);
  window.addEventListener(URL_EVENT, cb);
  return () => {
    window.removeEventListener("popstate", cb);
    window.removeEventListener(URL_EVENT, cb);
  };
}
const snapshot = (): string | null => new URLSearchParams(window.location.search).get(PARAM);
const serverSnapshot = (): string | null => null;

function useSelectedConversation(): [string | null, (id: string | null, mode?: "push" | "replace") => void] {
  const selected = useSyncExternalStore(subscribe, snapshot, serverSnapshot);
  const select = useCallback((id: string | null, mode: "push" | "replace" = "push") => {
    const url = new URL(window.location.href);
    if (id) url.searchParams.set(PARAM, id);
    else url.searchParams.delete(PARAM);
    const next = `${url.pathname}${url.search}${url.hash}`;
    if (next === `${window.location.pathname}${window.location.search}${window.location.hash}`) return;
    if (mode === "push") window.history.pushState(window.history.state, "", next);
    else window.history.replaceState(window.history.state, "", next);
    window.dispatchEvent(new Event(URL_EVENT));
  }, []);
  return [selected, select];
}

/**
 * The agent's Chat tab (D-102): a thin composition of the messaging panel from `components/chat`: the conversation
 * list (title filter, Active/Archived) and the thread of the selected conversation.
 */
export function ChatTab({
  agentId,
  wsId,
  base,
  orgId,
  agent,
  agentKind = "hosted",
  isDefault = false,
  terminalAvailable = false,
  onNavigate,
  statusSheetOpen = false,
  onStatusSheetOpenChange,
}: {
  agentId: string;
  wsId: string;
  base: string;
  orgId?: string;
  agent?: AgentIdent;
  /** The About part's kind badge. */
  agentKind?: string;
  isDefault?: boolean;
  /** The Terminal page is available (the Computer part offers it). */
  terminalAvailable?: boolean;
  /** Switch the agent page to another page (the rail's "Open computer", "Edit instructions"). */
  onNavigate?: (page: AgentPageId) => void;
  /** Phones: the status sheet (opened by the header's Status button or the banner). */
  statusSheetOpen?: boolean;
  onStatusSheetOpenChange?: (open: boolean) => void;
}) {
  const qc = useQueryClient();
  const direct = useDirectConversation(agentId);
  const [archived, setArchived] = useState(false);
  const [query, setQuery] = useState("");
  const list = useConversations(agentId, archived);
  // The rail's "Doing now" reads the active conversations even while the Archived list is shown (same query when not).
  const activeList = useConversations(agentId, false);
  const retention = useRetention(orgId);
  const [picked, select] = useSelectedConversation();
  const now = useMemo(() => new Date(), []);
  const canApprove = useCanApprove(wsId);
  const [railExpanded, setRailExpanded] = useRailExpanded();
  const [openTurn, setOpenTurn] = useState<OpenTurn>({ active: false, line: null });

  // (a response without an id is not a conversation: render the list alone rather than a row with no identity)
  const directConv = direct.data && typeof direct.data.id === "string" ? direct.data : null;
  const convs = useMemo(() => list.data?.items ?? [], [list.data]);
  const activeConvs = useMemo(() => {
    const items = activeList.data?.items ?? [];
    return directConv && !items.some((c) => c.id === directConv.id) ? [directConv, ...items] : items;
  }, [activeList.data, directConv]);
  // The conversation list carries `active_turn` but is not streamed: look again while the page is visible.
  useEffect(() => {
    const id = window.setInterval(() => {
      if (document.visibilityState !== "hidden") void qc.invalidateQueries({ queryKey: ["agents", agentId, "conversations", false] });
    }, 15_000);
    return () => window.clearInterval(id);
  }, [qc, agentId]);
  const groups: ListGroups = useMemo(() => {
    if (archived) return { mine: null, others: sortByActivity(filterConversations(convs, query)), memberDirects: [] };
    const all = directConv && !convs.some((c) => c.id === directConv.id) ? [directConv, ...convs] : convs;
    return groupConversations(filterConversations(all, query), directConv?.id ?? null);
  }, [archived, convs, directConv, query]);

  const ownId = directConv?.id ?? null;
  const convId = picked ?? (archived ? null : ownId);
  const inList = convId ? convs.some((c) => c.id === convId) : false;
  // A deep link to a conversation past the loaded pages: fetched by id so its title and kind still show (FIX-117).
  const fetched = useConversationById(convId, !!convId && convId !== ownId && list.isSuccess && !inList);
  const status = useAgentStatus({ wsId, agentId, canApprove, conversations: activeConvs, open: openTurn });
  const waitingIds = useMemo(
    () => (canApprove ? new Set(status.needs.flatMap((n) => (n.conversationId ? [n.conversationId] : []))) : undefined),
    [canApprove, status.needs],
  );

  if (direct.isPending) return <Loading />;
  if (direct.isError) return <ErrorState error={direct.error} onRetry={() => void direct.refetch()} />;

  const conv = convId
    ? (convs.find((c) => c.id === convId) ?? (convId === ownId ? directConv : null) ?? fetched.data ?? null)
    : null;
  // RFE-009: another member's direct conversation (owners only; the API returns it with `direct_user`) is read-only.
  const memberDirect = conv && conv.id !== ownId && conv.kind === "direct" && conv.direct_user ? conv : null;
  const done = () => {
    setArchived(false);
    select(null, "replace");
  };
  const threadOpen = picked !== null;
  const titleOf = (c: Conversation) => conversationTitle(c);
  const ctx: RailContext = {
    status,
    open: openTurn,
    wsId,
    base,
    agentId,
    agent: { kind: agentKind, isDefault, brief: agent?.brief },
    terminalAvailable,
    openConversation: convId ? { id: convId, title: conv ? conversationTitle(conv) : "Direct chat" } : null,
    titleOf,
    onSelectConversation: (id) => select(id),
    onNavigate: (page) => onNavigate?.(page),
  };
  const waiting = status.needs.length;

  return (
    <>
      {canApprove ? <ApprovalsLive wsId={wsId} /> : null}
      {canApprove && waiting > 0 ? (
        <button
          type="button"
          onClick={() => onStatusSheetOpenChange?.(true)}
          className="mb-2 flex min-h-11 w-full items-center justify-between gap-2 rounded-control border border-pending-line bg-pending-panel px-3 text-sm font-semibold text-pending-ink md:hidden"
        >
          <span>
            {waiting} {waiting === 1 ? "approval" : "approvals"} waiting
          </span>
          <ChevronRightIcon aria-hidden="true" width={16} height={16} />
        </button>
      ) : null}
      <StatusSheet ctx={ctx} open={statusSheetOpen} onOpenChange={(o) => onStatusSheetOpenChange?.(o)} />
    <ChatPanel
      threadOpen={threadOpen}
      railExpanded={railExpanded}
      rail={(overlay) => <StatusRail ctx={ctx} expanded={railExpanded} onExpandedChange={setRailExpanded} overlay={overlay} />}
      list={
        <ConversationList
          className="flex-1"
          waitingIds={waitingIds}
          groups={groups}
          selectedId={convId}
          archived={archived}
          onArchivedChange={(a) => {
            setArchived(a);
            select(null, "replace");
          }}
          query={query}
          onQueryChange={setQuery}
          onSelect={(id) => select(id)}
          now={now}
          status={list.isError ? <ErrorState error={list.error} onRetry={() => void list.refetch()} /> : list.isPending ? <Loading /> : undefined}
        />
      }
      thread={
        convId ? (
          <ChatThread
            key={convId}
            agent={agent}
            agentId={agentId}
            wsId={wsId}
            base={base}
            convId={convId}
            direct={convId === ownId ? directConv : null}
            conversation={conv}
            ownerView={memberDirect ? (memberDirect.direct_user?.display_name ?? "") : null}
            ownDirect={convId === ownId}
            archived={archived || !!conv?.archived_at}
            retentionDays={retention.data?.conversations_days ?? null}
            onGone={done}
            onBack={() => select(null)}
            onTurn={setOpenTurn}
          />
        ) : (
          <div className="flex flex-1 items-center justify-center p-6 text-center text-sm text-ink-2">
            {archived ? (
              <p>
                <span className="font-semibold text-ink">Archived conversations</span>
                <br />
                Pick one to read it, export it or unarchive it.
              </p>
            ) : (
              <p>Pick a conversation from the list.</p>
            )}
          </div>
        )
      }
    />
    </>
  );
}
