"use client";

import { useSearchParams } from "next/navigation";
import { useCallback, useEffect, useState } from "react";

import { useLeaveGate } from "@/components/agents/files/leave-gate";
import { EmptyState, ErrorState, Loading } from "@/components/states";
import { Button } from "@/components/ui/button";
import { PlusIcon } from "@/components/ui/icons";
import { HeaderActions, ListDetail, SubScreenHeader, useNarrowScreen } from "@/components/ui/list-detail";
import { SaveBar, useSaveBar } from "@/components/ui/save-bar";
import { SkeletonRows } from "@/components/ui/skeleton";
import { useMemories, useMemorySearch, type MemorySearchHit } from "@/lib/api/memory";
import { useMe } from "@/lib/api/queries";
import { clearMemoryDraft, peekMemoryDraft } from "@/lib/chat/memory-draft";

import { MemoryEditor } from "./memory-editor";
import { MemoryHistory } from "./memory-history";
import {
  FilterChips,
  MemoryRows,
  SearchField,
  SearchRows,
  SearchSummary,
  SortControl,
  matchesFilter,
  sortMemories,
  type MemoryFilter,
  type MemorySort,
} from "./memory-list";
import { MemoryView, type MemoryMatch } from "./memory-view";

/** How long after the last keystroke the search is sent. */
export const SEARCH_DEBOUNCE_MS = 250;

function useDebounced(value: string, ms: number): string {
  const [debounced, setDebounced] = useState(value);
  useEffect(() => {
    const t = setTimeout(() => setDebounced(value), ms);
    return () => clearTimeout(t);
  }, [value, ms]);
  return debounced;
}

/** The words a hit matched, as the server marked them in its snippet and title. */
export function matchedTerms(hit: MemorySearchHit): string[] {
  const from = (text: string, ranges: number[][]) => ranges.map(([s, e]) => text.slice(s ?? 0, e ?? 0));
  const terms = [...from(hit.snippet, hit.highlights), ...from(hit.title?.trim() ? hit.title : hit.key, hit.title_highlights)];
  return [...new Set(terms.map((t) => t.trim().toLowerCase()).filter(Boolean))];
}

/** `?tab=memory&memory=<key>[&history=1]` (C-1177), written with `replace`: a reload keeps the memory on screen. */
function writeAddress(key: string | null, history: boolean) {
  const url = new URL(window.location.href);
  if (key) url.searchParams.set("memory", key);
  else url.searchParams.delete("memory");
  if (key && history) url.searchParams.set("history", "1");
  else url.searchParams.delete("history");
  window.history.replaceState(window.history.state, "", `${url.pathname}${url.search}${url.hash}`);
}

type Form = { kind: "new" } | { kind: "edit"; key: string } | null;

export function MemoryTab({
  agentId,
  agentName = "your agent",
  readOnly = false,
}: {
  agentId: string;
  agentName?: string;
  /** Emergency read access: memories can be read, not searched or written. */
  readOnly?: boolean;
}) {
  const list = useMemories(agentId);
  const me = useMe().data?.user.id ?? null;
  const narrow = useNarrowScreen();
  const leave = useLeaveGate();
  const saved = useSaveBar();
  const params = useSearchParams();

  // ---- the address: the chosen memory and whether its history is open ----
  const read = (name: string) => params.get(name) ?? (typeof window === "undefined" ? null : new URLSearchParams(window.location.search).get(name));
  const [selected, setSelected] = useState<string | null>(() => read("memory"));
  const [historyOpen, setHistoryOpen] = useState(() => read("history") === "1");
  // A link that only changes `?memory=` (a "View" elsewhere on the page) is followed. What this page writes itself comes
  // back through the router too, and is not a link: while searching it must not replace the memory that Clear restores.
  const routerKey = params.get("memory");
  const [lastRouterKey, setLastRouterKey] = useState(routerKey);
  const [ownKey, setOwnKey] = useState(routerKey);
  if (routerKey !== lastRouterKey) {
    setLastRouterKey(routerKey);
    if (routerKey && routerKey !== ownKey && routerKey !== selected) {
      setSelected(routerKey);
      setHistoryOpen(params.get("history") === "1");
    }
  }
  const address = (key: string | null, history: boolean) => {
    setOwnKey(key);
    writeAddress(key, history);
  };

  // (P5-MEMORY-4) What the chat's **Save it yourself** hands over: the form opens with it, not saved.
  const [draft, setDraft] = useState(() => (readOnly ? null : peekMemoryDraft(agentId)));
  useEffect(() => {
    if (draft) clearMemoryDraft();
  }, [draft]);
  const [form, setForm] = useState<Form>(() => (draft ? (draft.kind === "edit" ? { kind: "edit", key: draft.key } : { kind: "new" }) : null));
  const [dirty, setDirty] = useState(false);
  const [filter, setFilter] = useState<MemoryFilter>("all");
  const [sort, setSort] = useState<MemorySort>("recent");

  // ---- search ----
  const [q, setQ] = useState("");
  const debounced = useDebounced(q.trim(), SEARCH_DEBOUNCE_MS);
  const searching = !readOnly && q.trim() !== "" && debounced !== "";
  const search = useMemorySearch(agentId, readOnly || q.trim() === "" ? "" : debounced);
  /** The result the person chose; the first one is shown until then. Clear leaves `selected` as it was. */
  const [hit, setHit] = useState<string | null>(null);

  const items = list.data?.items ?? [];
  const visible = sortMemories(
    items.filter((m) => matchesFilter(m, filter, me)),
    sort,
  );
  const hits = searching ? (search.data?.items ?? []) : [];

  const chosen = searching ? hit : selected;
  const known = items.some((m) => m.key === chosen);
  const fallback = searching ? (hits[0]?.key ?? null) : (visible[0]?.key ?? null);
  const currentKey = form?.kind === "edit" ? form.key : known ? chosen : fallback;
  const current = items.find((m) => m.key === currentKey) ?? null;
  const hitAt = hits.findIndex((h) => h.key === currentKey);
  const match: MemoryMatch | null =
    searching && hitAt >= 0 ? { terms: matchedTerms(hits[hitAt]!), index: hitAt + 1, total: hits.length } : null;

  // A draft for a memory that is gone starts a new one.
  const orphaned = draft?.kind === "edit" && form?.kind === "edit" && list.data !== undefined && !items.some((m) => m.key === form.key);
  const editing = form?.kind === "edit" && !orphaned ? current : null;
  const creating = form?.kind === "new" || orphaned;
  // On a narrow screen the memory (or the form) is its own screen once it was opened.
  const detailOpen = creating || editing !== null || (known && chosen !== null);

  const leaveForm = useCallback(() => {
    setDraft(null);
    setForm(null);
    setDirty(false);
  }, []);

  const choose = (key: string) =>
    leave.ask(() => {
      leaveForm();
      if (searching) setHit(key);
      else setSelected(key);
      setHistoryOpen(false);
      address(key, false);
    });
  const startNew = () =>
    leave.ask(() => {
      saved.clear();
      setDirty(false);
      setDraft(null);
      setForm({ kind: "new" });
    });
  const back = () =>
    leave.ask(() => {
      leaveForm();
      setHistoryOpen(false);
      if (searching) setHit(null);
      else setSelected(null);
      address(null, false);
    });
  const typeSearch = (value: string) => {
    setHit(null);
    setQ(value);
  };
  const clearSearch = () => {
    typeSearch("");
    address(selected, historyOpen);
  };
  const showHistory = (open: boolean) => {
    setHistoryOpen(open);
    if (currentKey) address(currentKey, open);
  };
  const afterSave = (key: string) => {
    const wasNew = form?.kind === "new";
    leaveForm();
    saved.flash();
    if (wasNew) {
      setQ("");
      setHit(null);
      setFilter("all");
    }
    setSelected(key);
    setHistoryOpen(false);
    address(key, false);
  };

  if (list.isPending) return <SkeletonRows rows={5} label="Loading memories…" />;
  if (list.isError) return <ErrorState error={list.error} onRetry={() => list.refetch()} />;

  const newButton = (
    <Button type="button" size="sm" className="max-md:min-h-11" aria-label="New memory" onClick={startNew}>
      <PlusIcon className="size-4" />
      <span className="max-md:hidden">New memory</span>
      <span className="md:hidden">New</span>
    </Button>
  );

  // (P5-POLISH-6, C6) No memory yet: the whole page is one empty state with its one action.
  if (items.length === 0 && !creating) {
    return (
      <EmptyState
        title={`${agentName} doesn't remember anything yet`}
        when={readOnly ? "It saves notes here as you work together." : "It saves notes here as you work together. You can also write one yourself."}
        action={
          readOnly ? undefined : (
            <Button type="button" onClick={startNew}>
              New memory
            </Button>
          )
        }
      />
    );
  }

  const listColumn = (
    <div className="min-w-0 space-y-3">
      {readOnly ? null : <SearchField value={q} onChange={typeSearch} />}
      {searching ? (
        <>
          {search.isPending ? <Loading label="Searching…" /> : null}
          {search.isError ? <ErrorState error={search.error} onRetry={() => search.refetch()} /> : null}
          {search.data ? <SearchSummary q={debounced} count={hits.length} onClear={clearSearch} /> : null}
          {hits.length ? (
            <SearchRows hits={hits} items={items} current={narrow ? null : currentKey} agentName={agentName} me={me} onChoose={choose} />
          ) : null}
        </>
      ) : (
        <>
          <FilterChips items={items} filter={filter} onFilter={setFilter} agentName={agentName} me={me} />
          <SortControl sort={sort} onSort={setSort} />
          {visible.length ? (
            <MemoryRows
              items={visible}
              current={narrow ? null : creating ? null : currentKey}
              dirtyKey={dirty && editing ? editing.key : null}
              agentName={agentName}
              me={me}
              onChoose={choose}
            />
          ) : items.length ? (
            <p className="text-sm text-muted">No memories here.</p>
          ) : null}
        </>
      )}
    </div>
  );

  // While a search has no result to show, nothing unrelated is shown beside it.
  const shown = searching && hitAt < 0 && !editing ? null : current;
  const detail = creating ? (
    <MemoryEditor
      key="new"
      draft={draft ?? undefined}
      agentId={agentId}
      agentName={agentName}
      taken={items.map((m) => m.key)}
      narrow={narrow}
      onDirtyChange={setDirty}
      onSaved={afterSave}
      onClose={leaveForm}
      onBack={back}
    />
  ) : editing && !readOnly ? (
    <MemoryEditor
      key={editing.key}
      draft={draft?.kind === "edit" && draft.key === editing.key ? draft : undefined}
      agentId={agentId}
      agentName={agentName}
      memory={editing}
      taken={[]}
      narrow={narrow}
      onDirtyChange={setDirty}
      onSaved={afterSave}
      onClose={leaveForm}
      onBack={back}
    />
  ) : shown && historyOpen && !readOnly ? (
    // History takes the reading view's place. An emergency reader has no history route.
    <MemoryHistory
      key={shown.key}
      agentId={agentId}
      memory={shown}
      agentName={agentName}
      narrow={narrow}
      onBack={() => showHistory(false)}
      onRestored={() => showHistory(false)}
    />
  ) : shown ? (
    <MemoryView
      key={shown.key}
      agentId={agentId}
      memory={shown}
      agentName={agentName}
      me={me}
      readOnly={readOnly}
      narrow={narrow}
      match={match}
      onBack={back}
      onEdit={() => {
        saved.clear();
        setForm({ kind: "edit", key: shown.key });
      }}
      onHistory={() => showHistory(true)}
      onDeleted={() => {
        setSelected(null);
        setHit(null);
        setHistoryOpen(false);
        address(null, false);
      }}
    />
  ) : narrow ? (
    <SubScreenHeader backLabel="Memory" onBack={back} />
  ) : null;

  return (
    <div className="min-w-0">
      {readOnly || (narrow && detailOpen) ? null : <HeaderActions>{newButton}</HeaderActions>}
      <ListDetail narrow={narrow} detailOpen={detailOpen} list={listColumn} detail={detail} />
      {leave.dialog}
      {form ? null : <SaveBar dirty={false} saved={saved.saved} onDiscard={() => undefined} />}
    </div>
  );
}
