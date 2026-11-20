"use client";

// (P5-REFINE-16 item 2 part B, D-415) The workspace Activity page's live timeline: the per-agent feeds of the workspace's
// agents merged newest first. There is no workspace-level timeline route (P5-REFINE-9 builds the per-agent one only), so
// "Any agent" reads each agent's feed and merges them here.
//
// Each agent's first page is cached under the same keys `useTimelineStream` merges its `row` upserts into (the data is
// `InfiniteData`-shaped), so a new row lands on top and a running row turns into its outcome in place with no code here.
// A person holds at most five streams (`TIMELINE_STREAMS_PER_USER`): the page keeps up to `MAX_LIVE_AGENTS` of them open
// and polls the feeds of the other agents.

import { useQueries, useQueryClient } from "@tanstack/react-query";
import { useCallback, useMemo } from "react";

import { fetchTimelinePage, timelineKeys, type TimelineData, type TimelineFilters, type TimelineItem } from "./timeline";

/** Streams the Activity page opens (the rest of a person's five stay free for the agent page's rail). */
export const MAX_LIVE_AGENTS = 4;
/** Agents whose feeds the page reads at most. */
export const MAX_AGENTS = 20;
const PAGE = 25;
const POLL_MS = 30_000;

const feedKey = (agentId: string, f: TimelineFilters) => [...timelineKeys.list(agentId, f), "feed"] as const;

export interface WorkspaceTimeline {
  items: TimelineItem[];
  loading: boolean;
  error: unknown;
  hasMore: boolean;
  loadMore: () => Promise<void>;
  refetch: () => void;
  /** Agents that have no stream (more than `MAX_LIVE_AGENTS`): their feeds are polled. */
  polled: ReadonlySet<string>;
}

/** The agents' feeds merged, newest first (the same `(at, id)` order as the API's keyset). */
export function useWorkspaceTimeline(agentIds: readonly string[], filters: TimelineFilters, enabled = true): WorkspaceTimeline {
  const qc = useQueryClient();
  const ids = useMemo(() => agentIds.slice(0, MAX_AGENTS), [agentIds]);
  const live = useMemo(() => new Set(ids.slice(0, MAX_LIVE_AGENTS)), [ids]);
  const polled = useMemo(() => new Set(ids.filter((id) => !live.has(id))), [ids, live]);
  const results = useQueries({
    queries: ids.map((id) => ({
      queryKey: feedKey(id, filters),
      queryFn: async (): Promise<TimelineData> => ({ pages: [await fetchTimelinePage(id, filters, PAGE)], pageParams: [undefined] }),
      enabled,
      retry: false,
      refetchInterval: live.has(id) ? (false as const) : POLL_MS,
    })),
  });

  // A few hundred rows at most: merged on each render.
  const items = results
    .flatMap((r) => r.data?.pages.flatMap((p) => p.items) ?? [])
    .sort((a, b) => (a.at === b.at ? (a.id < b.id ? 1 : -1) : a.at < b.at ? 1 : -1));

  const cursorOf = (data: TimelineData | undefined) => data?.pages[data.pages.length - 1]?.next_cursor ?? undefined;
  const hasMore = results.some((r) => !!cursorOf(r.data));

  const loadMore = useCallback(async () => {
    await Promise.all(
      ids.map(async (id) => {
        const key = feedKey(id, filters);
        const cursor = cursorOf(qc.getQueryData<TimelineData>(key));
        if (!cursor) return;
        const page = await fetchTimelinePage(id, filters, PAGE, cursor);
        qc.setQueryData<TimelineData>(key, (d) => (d ? { pages: [...d.pages, page], pageParams: [...d.pageParams, cursor] } : d));
      }),
    );
  }, [qc, ids, filters]);

  return {
    items,
    loading: enabled && ids.length > 0 && results.some((r) => r.isPending),
    error: results.find((r) => r.isError)?.error ?? null,
    hasMore,
    loadMore,
    refetch: () => results.forEach((r) => void r.refetch()),
    polled,
  };
}
