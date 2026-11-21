"use client";

// (P5-REFINE-9, D-415; C-1083, C-1084) The agent timeline's typed client: the paged feed and its live
// stream. No UI here: P5-REFINE-11 (the rail's "What <Agent> did") and P5-REFINE-16 (Activity, the
// Technical view) draw it, and write the sentences from `lib/labels.ts` — the API returns kinds and names.

import { useInfiniteQuery, useQueryClient, type InfiniteData, type QueryClient } from "@tanstack/react-query";
import { useCallback, useEffect, useRef, useState } from "react";

import { api } from "./client";
import { setRateNotice } from "./rate-limit";
import type { components } from "./schema";
import { unwrap } from "./unwrap";

type S = components["schemas"];
export type TimelineItem = S["TimelineItem"];
export type TimelineKind = S["TimelineKind"];
export type TimelineState = S["TimelineState"];
export type TimelinePage = S["TimelinePage"];
export type TimelineRowEvent = S["TimelineRowEvent"];
export type TimelineRequestEvent = S["TimelineRequestEvent"];

export interface TimelineFilters {
  kinds?: TimelineKind[];
  outcomes?: string[];
}

export type TimelineData = InfiniteData<TimelinePage, string | undefined>;

const key = (f: TimelineFilters) => [(f.kinds ?? []).join(","), (f.outcomes ?? []).join(",")] as const;

export const timelineKeys = {
  all: (agentId: string) => ["timeline", agentId] as const,
  list: (agentId: string, f: TimelineFilters = {}) => ["timeline", agentId, ...key(f)] as const,
};

/** One page of an agent's timeline (`cursor`: the previous page's `next_cursor`). */
export function fetchTimelinePage(agentId: string, filters: TimelineFilters, limit: number, cursor?: string): Promise<TimelinePage> {
  return unwrap(
    api.GET("/v1/agents/{agent}/timeline", {
      params: {
        path: { agent: agentId },
        query: {
          limit,
          cursor,
          kind: filters.kinds?.length ? filters.kinds.join(",") : undefined,
          outcome: filters.outcomes?.length ? filters.outcomes.join(",") : undefined,
        },
      },
    }),
  );
}

/** One agent's timeline, newest first, paged on `next_cursor`. */
export function useAgentTimeline(agentId: string, filters: TimelineFilters = {}, limit = 50) {
  return useInfiniteQuery({
    queryKey: timelineKeys.list(agentId, filters),
    initialPageParam: undefined as string | undefined,
    queryFn: ({ pageParam }) => fetchTimelinePage(agentId, filters, limit, pageParam),
    getNextPageParam: (last) => last.next_cursor ?? undefined,
    enabled: !!agentId,
    retry: false,
  });
}

/** Whether a row belongs in a list with these filters (the server applies the same rule to pages). */
export function matchesFilters(item: TimelineItem, f: TimelineFilters): boolean {
  if (f.kinds?.length && !f.kinds.includes(item.kind)) return false;
  if (f.outcomes?.length && !(item.outcome != null && f.outcomes.includes(item.outcome))) return false;
  return true;
}

/** Is a row with this id already in the cached pages? */
export function hasRow(data: TimelineData | undefined, id: string): boolean {
  return !!data?.pages.some((p) => p.items.some((i) => i.id === id));
}

/**
 * Merge one upsert into the cached pages: a row with the same `id` is replaced where it is (a "Running…"
 * row turns into its outcome in place); a new `id` goes to the top of the first page. Nothing cached yet:
 * unchanged (the first fetch brings it).
 */
export function mergeUpsert(data: TimelineData | undefined, item: TimelineItem): TimelineData | undefined {
  if (!data || data.pages.length === 0) return data;
  if (hasRow(data, item.id)) {
    return {
      ...data,
      pages: data.pages.map((p) =>
        p.items.some((i) => i.id === item.id) ? { ...p, items: p.items.map((i) => (i.id === item.id ? item : i)) } : p,
      ),
    };
  }
  const [first, ...rest] = data.pages;
  return { ...data, pages: [{ ...first, items: [item, ...first.items] }, ...rest] };
}

/** The `data` of a `row` event; `null` for anything malformed. */
export function parseRowEvent(text: string): TimelineRowEvent | null {
  try {
    const v = JSON.parse(text) as Partial<TimelineRowEvent>;
    if (v?.op !== "upsert" || !v.item || typeof v.item.id !== "string") return null;
    return v as TimelineRowEvent;
  } catch {
    return null;
  }
}

function parseRequest(text: string): TimelineRequestEvent | null {
  try {
    const v = JSON.parse(text) as Partial<TimelineRequestEvent>;
    return typeof v?.id === "string" ? (v as TimelineRequestEvent) : null;
  } catch {
    return null;
  }
}

function applyToLists(qc: QueryClient, agentId: string, item: TimelineItem, onlyExisting: boolean) {
  for (const [qk, data] of qc.getQueriesData<TimelineData>({ queryKey: timelineKeys.all(agentId) })) {
    const f: TimelineFilters = {
      kinds: (qk[2] as string | undefined)?.split(",").filter(Boolean) as TimelineKind[] | undefined,
      outcomes: (qk[3] as string | undefined)?.split(",").filter(Boolean),
    };
    const known = hasRow(data, item.id);
    if (onlyExisting && !known) continue;
    if (!known && !matchesFilters(item, f)) continue;
    qc.setQueryData<TimelineData>(qk, (d) => mergeUpsert(d, item));
  }
}

export interface TimelineStreamOptions {
  /** Also receive the Technical view's `request` events (`?technical=true`). */
  technical?: boolean;
  /**
   * Whether the list is scrolled to the top. While it is not, new rows wait (`pendingNew`) so the list does
   * not move under the reader; rows already shown still update in place.
   */
  atTop?: boolean;
  /** Each new request-log entry while `technical` is on. */
  onRequest?: (entry: TimelineRequestEvent) => void;
}

export interface TimelineStream {
  live: boolean;
  /** Rows that arrived while the list was not at the top ("3 new · Show"). */
  pendingNew: number;
  /** Put the waiting rows at the top. */
  showNew: () => void;
}

/**
 * The agent's live timeline (C-1084): one `EventSource` per agent; `row` upserts merge into every cached list
 * of the agent by `id`, `resync` (and a reconnect: the stream has no ids) refetches.
 */
export function useTimelineStream(agentId: string, opts: TimelineStreamOptions = {}): TimelineStream {
  const qc = useQueryClient();
  const [live, setLive] = useState(false);
  const [pendingNew, setPendingNew] = useState(0);
  const waiting = useRef<TimelineItem[]>([]);
  const atTop = useRef(opts.atTop ?? true);
  const onRequest = useRef(opts.onRequest);
  useEffect(() => {
    onRequest.current = opts.onRequest;
  }, [opts.onRequest]);
  const technical = !!opts.technical;

  const showNew = useCallback(() => {
    const rows = waiting.current;
    waiting.current = [];
    setPendingNew(0);
    // Oldest first, so the newest ends on top.
    for (const item of [...rows].reverse()) applyToLists(qc, agentId, item, false);
  }, [qc, agentId]);

  useEffect(() => {
    atTop.current = opts.atTop ?? true;
    if (atTop.current && waiting.current.length > 0) showNew();
  }, [opts.atTop, showNew]);

  useEffect(() => {
    if (!agentId || typeof EventSource === "undefined") return;
    const es = new EventSource(`/v1/agents/${agentId}/timeline/stream${technical ? "?technical=true" : ""}`);
    let opened = false;
    const refetch = () => {
      waiting.current = [];
      setPendingNew(0);
      void qc.invalidateQueries({ queryKey: timelineKeys.all(agentId) });
    };
    es.onopen = () => {
      setLive(true);
      if (opened) refetch();
      opened = true;
    };
    es.onerror = () => {
      setLive(false);
      // A refused connection (429 `rate_limited`: too many timelines open) is CLOSED, not retried.
      if (es.readyState === EventSource.CLOSED) setRateNotice({ kind: "streams" });
    };
    es.addEventListener("resync", refetch);
    es.addEventListener("row", (e) => {
      const evt = parseRowEvent((e as MessageEvent<string>).data);
      if (!evt) return;
      const item = evt.item;
      const shown = qc
        .getQueriesData<TimelineData>({ queryKey: timelineKeys.all(agentId) })
        .some(([, d]) => hasRow(d, item.id));
      if (shown) {
        applyToLists(qc, agentId, item, true);
        // A row that is also waiting is replaced there.
        waiting.current = waiting.current.map((w) => (w.id === item.id ? item : w));
        return;
      }
      if (atTop.current) {
        applyToLists(qc, agentId, item, false);
        return;
      }
      const i = waiting.current.findIndex((w) => w.id === item.id);
      if (i >= 0) waiting.current[i] = item;
      else {
        waiting.current = [item, ...waiting.current];
        setPendingNew(waiting.current.length);
      }
    });
    es.addEventListener("request", (e) => {
      const entry = parseRequest((e as MessageEvent<string>).data);
      if (entry) onRequest.current?.(entry);
    });
    return () => {
      es.close();
      setLive(false);
    };
  }, [qc, agentId, technical]);

  return { live, pendingNew, showNew };
}
