"use client";

import type { UseInfiniteQueryResult, InfiniteData } from "@tanstack/react-query";
import Link from "next/link";
import { useEffect, useRef, type MouseEvent } from "react";

import { KindTile } from "@/components/chat/conversation-list";
import { ErrorState, Loading } from "@/components/states";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { HighlightedText } from "@/components/ui/highlighted-text";
import { ClockIcon } from "@/components/ui/icons";
import { Segmented } from "@/components/ui/segmented";
import type { SearchItem, SearchResponse } from "@/lib/api/search";
import { KIND_LABEL, listTime } from "@/lib/chat/layout";
import { cn } from "@/lib/utils";

export type SearchScope = "agent" | "all";
export type SearchSort = "recent" | "relevance";

/** The id a result is selected by (the same message can match once only, but a conversation has many matches). */
export const resultKey = (i: Pick<SearchItem, "seq"> & { conversation: { id: string } }) => `${i.conversation.id}:${i.seq}`;

/**
 * The list pane while a message search shows (P5-CHAT-4 part B): scope, the count and sort, the "still being indexed"
 * line, and the result rows. A row is a link to `?c=&m=`; a click on a result of this agent opens it in place
 * (`onOpen`), one of another agent follows the link.
 */
export function SearchResults({
  query,
  text,
  scope,
  onScopeChange,
  sort,
  onSortChange,
  now,
  selectedKey,
  hrefOf,
  isLocal,
  onOpen,
  className,
}: {
  query: UseInfiniteQueryResult<InfiniteData<SearchResponse>, Error>;
  /** The submitted words, for the empty state. */
  text: string;
  scope: SearchScope;
  onScopeChange: (s: SearchScope) => void;
  sort: SearchSort;
  onSortChange: (s: SearchSort) => void;
  now: Date;
  selectedKey: string | null;
  hrefOf: (i: SearchItem) => string;
  /** The result belongs to the agent whose page this is. */
  isLocal: (i: SearchItem) => boolean;
  onOpen: (i: SearchItem) => void;
  className?: string;
}) {
  const items = query.data?.pages.flatMap((p) => p.items) ?? [];
  const more = query.hasNextPage;
  const incomplete = query.data?.pages.some((p) => !p.index_complete) ?? false;
  const sentinel = useRef<HTMLDivElement>(null);
  const hasIO = typeof IntersectionObserver !== "undefined";
  const { fetchNextPage, isFetchingNextPage, isFetchNextPageError } = query;
  useEffect(() => {
    const el = sentinel.current;
    if (!el || !more || !hasIO || isFetchingNextPage || isFetchNextPageError) return;
    const io = new IntersectionObserver((entries) => {
      if (entries.some((e) => e.isIntersecting)) void fetchNextPage();
    });
    io.observe(el);
    return () => io.disconnect();
  }, [more, hasIO, isFetchingNextPage, isFetchNextPageError, fetchNextPage, items.length]);

  return (
    <div className={cn("flex min-h-0 flex-1 flex-col gap-2", className)}>
      <div className="flex items-center gap-2">
        <span aria-hidden="true" className="shrink-0 text-xs text-muted">
          Search in
        </span>
        <Segmented<SearchScope>
          label="Search in"
          value={scope}
          onChange={onScopeChange}
          options={[
            { value: "agent", label: "This agent" },
            { value: "all", label: "All agents" },
          ]}
          className="min-w-0 [&_span]:px-2"
        />
      </div>
      {query.isPending ? <Loading label="Searching…" /> : null}
      {query.isError && !(query.error as { code?: string }).code?.startsWith("search_query") ? (
        <ErrorState error={query.error} onRetry={() => void query.refetch()} />
      ) : null}
      {query.isSuccess ? (
        <>
          <div className="flex items-center justify-between gap-2">
            <p role="status" className="text-xs font-medium text-ink-2">
              {items.length}
              {more ? "+" : ""} {items.length === 1 && !more ? "result" : "results"}
            </p>
            <select
              aria-label="Sort results"
              value={sort}
              onChange={(e) => onSortChange(e.target.value as SearchSort)}
              className="min-h-11 rounded-control border border-line-strong bg-surface px-2 text-xs text-ink md:min-h-8 dark:bg-canvas">
              <option value="recent">Newest first</option>
              <option value="relevance">Best match</option>
            </select>
          </div>
          {incomplete ? (
            <p className="flex items-center gap-1.5 text-xs text-muted">
              <ClockIcon aria-hidden="true" width={12} height={12} />
              Older messages are still being indexed
            </p>
          ) : null}
          {items.length === 0 ? (
            <div className="px-1 py-2 text-sm text-ink-2">
              <p>No messages match “{text}”.</p>
              <p className="mt-1 text-xs text-muted">Try fewer words{scope === "agent" ? ", or search All agents" : ""}.</p>
            </div>
          ) : (
            <ul aria-label="Search results" className="-mx-1 min-h-0 flex-1 space-y-0.5 overflow-y-auto px-1">
              {items.map((i) => (
                <li key={resultKey(i)}>
                  <ResultRow item={i} now={now} showAgent={scope === "all"} selected={selectedKey === resultKey(i)} href={hrefOf(i)} local={isLocal(i)} onOpen={() => onOpen(i)} />
                </li>
              ))}
              {more ? (
                <li>
                  <div ref={sentinel} className="py-1 text-center">
                    {isFetchNextPageError ? (
                      <Button variant="outline" size="sm" onClick={() => void fetchNextPage()}>
                        Try again
                      </Button>
                    ) : !hasIO ? (
                      <Button variant="outline" size="sm" onClick={() => void fetchNextPage()} disabled={isFetchingNextPage}>
                        Load more
                      </Button>
                    ) : isFetchingNextPage ? (
                      <Loading label="Loading more…" />
                    ) : null}
                  </div>
                </li>
              ) : null}
            </ul>
          )}
        </>
      ) : null}
    </div>
  );
}

/** One result: the conversation's tile, title and time, the author, and the snippet with its highlights. */
export function ResultRow({
  item: i,
  now,
  showAgent,
  selected,
  href,
  local,
  onOpen,
}: {
  item: SearchItem;
  now: Date;
  showAgent: boolean;
  selected: boolean;
  href: string;
  local: boolean;
  onOpen: () => void;
}) {
  const click = (e: MouseEvent<HTMLAnchorElement>) => {
    // modified clicks keep the link's own behaviour (new tab, new window)
    if (!local || e.defaultPrevented || e.button !== 0 || e.metaKey || e.ctrlKey || e.shiftKey || e.altKey) return;
    e.preventDefault();
    onOpen();
  };
  const author = [i.author.label, showAgent ? i.agent.name : null].filter(Boolean).join(" · ");
  return (
    <Link
      href={href}
      onClick={click}
      aria-current={selected ? "true" : undefined}
      className={cn("flex min-h-11 w-full items-start gap-2 rounded-control p-2 text-left hover:bg-subtle", selected && "bg-subtle")}>
      <KindTile kind={i.conversation.kind} size={28} className="mt-0.5" />
      <span className="min-w-0 flex-1">
        <span className="flex items-center gap-2">
          <span className="min-w-0 flex-1 truncate text-sm font-semibold text-ink">{i.conversation.title || kindTitle(i.conversation.kind)}</span>
          {i.conversation.archived ? <Badge>Archived</Badge> : null}
          <span className="shrink-0 text-xs tabular-nums text-muted">{listTime(i.at, now)}</span>
        </span>
        <span className="block truncate text-xs text-muted">
          {author}
          {i.type === "attachment" ? " · File" : ""}
        </span>
        <HighlightedText text={i.snippet} highlights={i.highlights} className="block line-clamp-3 text-[13px] leading-[19px] text-ink-2" />
      </span>
    </Link>
  );
}

const kindTitle = (kind: string) => KIND_LABEL[kind] ?? kind;
