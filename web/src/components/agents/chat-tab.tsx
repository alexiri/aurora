"use client";

import { useQueryClient } from "@tanstack/react-query";
import { useSearchParams } from "next/navigation";
import { useCallback, useEffect, useMemo, useRef, useState, useSyncExternalStore, type KeyboardEvent } from "react";

import { ChatPanel } from "@/components/chat/chat-panel";
import { SearchBox } from "@/components/chat/search/search-box";
import { resultKey, SearchResults, type SearchScope, type SearchSort } from "@/components/chat/search/search-results";
import { ApiError } from "@/lib/api/client";
import { errorDetail } from "@/lib/api/errors";
import { useAgents, useOrg } from "@/lib/api/queries";
import { useMessageSearch, type SearchItem } from "@/lib/api/search";
import { extractFilters, queryOf, toSearchParams, type FilterChip } from "@/lib/chat/search-syntax";
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

function useSelectedConversation(): [string | null, (id: string | null, mode?: "push" | "replace", seq?: number) => void] {
  const selected = useSyncExternalStore(subscribe, snapshot, serverSnapshot);
  // `seq` makes it a permalink (`?c=&m=`, P5-CHAT-2): the thread jumps to that message and highlights it.
  const select = useCallback((id: string | null, mode: "push" | "replace" = "push", seq?: number) => {
    const url = new URL(window.location.href);
    if (id) url.searchParams.set(PARAM, id);
    else url.searchParams.delete(PARAM);
    if (id && seq !== undefined) url.searchParams.set("m", String(seq));
    else url.searchParams.delete("m");
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
  sshAvailable = false,
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
  /** (P4-SSH-2, C-795) The SSH page is available (the Computer part links to it). */
  sshAvailable?: boolean;
  /** Switch the agent page to another page (the rail's "Open computer", "Edit instructions"). */
  onNavigate?: (page: AgentPageId) => void;
  /** Phones: the status sheet (opened by the header's Status button or the banner). */
  statusSheetOpen?: boolean;
  onStatusSheetOpenChange?: (open: boolean) => void;
}) {
  const qc = useQueryClient();
  const direct = useDirectConversation(agentId);
  const [archived, setArchived] = useState(false);
  // (P5-CHAT-4) The search box: `text` filters titles as you type; Enter submits a message search (`submitted`).
  const [query, setQuery] = useState("");
  const [chips, setChips] = useState<FilterChip[]>([]);
  const [submitted, setSubmitted] = useState<{ q: string; chips: FilterChip[] } | null>(null);
  const [scope, setScope] = useState<SearchScope>("agent");
  const [sort, setSort] = useState<SearchSort>("recent");
  const [selectedResult, setSelectedResult] = useState<string | null>(null);
  const [openNonce, setOpenNonce] = useState(0);
  const searchInput = useRef<HTMLInputElement>(null);
  const params = useMemo(
    () => (submitted ? toSearchParams(submitted.q, submitted.chips, { agentId: scope === "agent" ? agentId : undefined, sort }) : null),
    [submitted, scope, sort, agentId],
  );
  const search = useMessageSearch(wsId, params);
  // Other agents' results link to their own page: their handles come from the workspace's agent list.
  const agents = useAgents(submitted && scope === "all" ? wsId : undefined);
  const list = useConversations(agentId, archived);
  // The rail's "Doing now" reads the active conversations even while the Archived list is shown (same query when not).
  const activeList = useConversations(agentId, false);
  // The retention rule is for organization owners and admins (a member gets 403): ask only for them.
  const orgRole = useOrg(orgId).data?.my_role;
  const retention = useRetention(orgId, orgRole === "owner" || orgRole === "admin");
  const [pickedRaw, selectRaw] = useSelectedConversation();
  // (P5-REFINE-13, C-1035) `?draft=` is read from the router (not `window.location`, which still shows the previous
  // page during a client-side navigation) and kept here until the person edits the box or sends, so a remount of the
  // composer (the direct chat resolving, the transcript loading) cannot lose it. A draft opens the direct chat.
  // `useSearchParams()` is null outside the app router (tests rendering the tab alone), as approvals-view handles.
  const routerDraft = useSearchParams()?.get("draft") ?? null;
  const [draft, setDraft] = useState<string | null>(null);
  const [seenDraft, setSeenDraft] = useState<string | null>(null);
  if (routerDraft !== seenDraft) {
    setSeenDraft(routerDraft);
    if (routerDraft) setDraft(routerDraft.slice(0, 500));
  }
  const picked = draft !== null ? null : pickedRaw;
  const select = useCallback(
    (id: string | null, mode?: "push" | "replace", seq?: number) => {
      setDraft(null);
      selectRaw(id, mode, seq);
    },
    [selectRaw],
  );
  const onDraftUsed = useCallback(() => setDraft(null), []);
  useEffect(() => {
    if (draft !== null && pickedRaw) selectRaw(null, "replace");
  }, [draft, pickedRaw, selectRaw]);
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

  const searchError = (() => {
    const e = search.error;
    if (!(e instanceof ApiError) || e.code !== "search_query_invalid") return null;
    const reason = errorDetail(e, "reason");
    return reason === "too_long" ? "Searches are limited to 200 characters." : reason === "too_many_terms" ? "Use at most 12 words." : "Type something to search for.";
  })();
  /** Enter: the finished filters become chips, the rest is `q`; nothing is sent without words. */
  const runSearch = (text: string, current: FilterChip[]) => {
    const r = extractFilters(text, current, true);
    setChips(r.chips);
    setQuery(r.rest);
    const q = queryOf(r.rest);
    if (!q) return;
    setSelectedResult(null);
    setSubmitted({ q, chips: r.chips });
  };
  const clearSearch = () => {
    setQuery("");
    setChips([]);
    setSubmitted(null);
    setSelectedResult(null);
  };
  const changeChips = (next: FilterChip[]) => {
    setChips(next);
    // removing a chip of a running search runs it again without that filter
    if (submitted && next.length < chips.length) setSubmitted({ ...submitted, chips: next });
  };
  const handleOf = (id: string) => agents.data?.items.find((a) => a.id === id)?.handle;
  const hrefOf = (i: SearchItem) => {
    const q = `?c=${i.conversation.id}&m=${i.seq}`;
    if (i.agent.id === agentId) return q;
    const handle = handleOf(i.agent.id);
    return handle ? `${base}/agents/${handle}${q}` : `${base}/agents`;
  };
  const openResult = (i: SearchItem) => {
    setSelectedResult(resultKey(i));
    select(i.conversation.id, "push", i.seq);
    setOpenNonce((n) => n + 1);
  };
  // `/` (not in a text field) and Ctrl/Cmd+F, with focus anywhere in the panel, go to the search box.
  const panelKey = (e: KeyboardEvent<HTMLDivElement>) => {
    if (e.defaultPrevented) return;
    const typing = (e.target as HTMLElement).closest("input, textarea, select, [contenteditable]:not([contenteditable='false'])");
    const plain = !e.ctrlKey && !e.metaKey && !e.altKey;
    if ((e.key === "/" && plain && !typing) || ((e.ctrlKey || e.metaKey) && !e.altKey && !e.shiftKey && e.key.toLowerCase() === "f")) {
      e.preventDefault();
      searchInput.current?.focus();
      searchInput.current?.select();
    }
  };

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
    agent: { name: agent?.name, kind: agentKind, isDefault, brief: agent?.brief },
    terminalAvailable,
    sshAvailable,
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
      listWide={!!submitted}
      onKeyDown={panelKey}
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
          searchBox={
            <SearchBox
              text={query}
              onTextChange={setQuery}
              chips={chips}
              onChipsChange={changeChips}
              conversations={convs}
              onSelectConversation={(id) => {
                setQuery("");
                select(id);
              }}
              onSubmit={runSearch}
              onClear={clearSearch}
              inputRef={searchInput}
              error={searchError}
              searching={!!submitted}
            />
          }
          results={
            submitted ? (
              <SearchResults
                query={search}
                text={submitted.q}
                scope={scope}
                onScopeChange={setScope}
                sort={sort}
                onSortChange={setSort}
                now={now}
                selectedKey={selectedResult}
                hrefOf={hrefOf}
                isLocal={(i) => i.agent.id === agentId}
                onOpen={openResult}
              />
            ) : undefined
          }
          onSelect={(id) => select(id)}
          now={now}
          status={list.isError ? <ErrorState error={list.error} onRetry={() => void list.refetch()} /> : list.isPending ? <Loading /> : undefined}
        />
      }
      thread={
        convId ? (
          <ChatThread
            key={`${convId}:${openNonce}`}
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
            initialDraft={draft ?? undefined}
            onDraftUsed={onDraftUsed}
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
