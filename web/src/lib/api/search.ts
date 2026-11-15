"use client";

// (P5-CHAT-4 part B, C-727) Message search over the generated client: pages follow `next_cursor`.
import { useInfiniteQuery } from "@tanstack/react-query";

import type { SearchParams } from "@/lib/chat/search-syntax";

import { api } from "./client";
import type { components } from "./schema";
import { unwrap } from "./unwrap";

export type SearchItem = components["schemas"]["SearchItem"];
export type SearchResponse = components["schemas"]["SearchResponse"];

const PAGE = 20;

/** The pages of one search; `params === null` runs nothing. A 422 `search_query_invalid` surfaces as the query's error. */
export function useMessageSearch(wsId: string, params: SearchParams | null) {
  return useInfiniteQuery({
    queryKey: ["messages-search", wsId, params] as const,
    enabled: !!params && !!wsId,
    initialPageParam: undefined as string | undefined,
    queryFn: ({ pageParam }) =>
      unwrap(api.GET("/v1/workspaces/{ws}/messages/search", { params: { path: { ws: wsId }, query: { ...params!, limit: PAGE, cursor: pageParam } } })),
    getNextPageParam: (last) => last.next_cursor ?? undefined,
    retry: false,
    staleTime: 30_000,
  });
}
