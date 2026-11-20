"use client";

import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect } from "react";

import { api } from "./client";
import type { components } from "./schema";
import { unwrap } from "./unwrap";

export type AttentionItem = components["schemas"]["AttentionItem"];
export type AttentionList = components["schemas"]["AttentionList"];
export type AttentionCounts = components["schemas"]["AttentionCounts"];

export const attentionKey = ["admin", "attention"] as const;

/** (P5-REFINE-8, C-1080/C-1082) The feeds and the counts share the `["attention"]` prefix: one invalidation refreshes both. */
export const attentionKeys = {
  all: ["attention"] as const,
  feed: (level: AttentionLevel, id: string, state: AttentionFeedState) => ["attention", "feed", level, id, state] as const,
  counts: (org: string | undefined, workspace: string | undefined) => ["attention", "counts", org ?? "", workspace ?? ""] as const,
};

export type AttentionLevel = "workspace" | "organization" | "administration";
export type AttentionFeedState = "open" | "done";

/**
 * (P5-REFINE-8, C-1080) One level's attention feed: `GET /v1/workspaces/{ws}/attention`, `/v1/orgs/{org}/attention` or
 * `/v1/admin/attention` (`id` is ignored for Administration). `count` is what the rail shows (open items with
 * `own: false`); `state: "done"` lists what closed in the last 30 days.
 */
export function useAttentionFeed(level: AttentionLevel, id: string | undefined, state: AttentionFeedState = "open", enabled = true) {
  return useQuery({
    queryKey: attentionKeys.feed(level, id ?? "", state),
    enabled: enabled && (level === "administration" || !!id),
    retry: false,
    staleTime: 30_000,
    refetchOnWindowFocus: true,
    queryFn: () => {
      const query = { state };
      switch (level) {
        case "workspace":
          return unwrap(api.GET("/v1/workspaces/{ws}/attention", { params: { path: { ws: id! }, query } }));
        case "organization":
          return unwrap(api.GET("/v1/orgs/{org}/attention", { params: { path: { org: id! }, query } }));
        default:
          return unwrap(api.GET("/v1/admin/attention", { params: { query } }));
      }
    },
  });
}

/**
 * (P5-REFINE-8, C-1082) The rails' numbers for the scopes asked for: each is `null` when the caller cannot act there
 * (never an error). Refetched on window focus and every 5 minutes; deciding, declining or resolving an item
 * invalidates `attentionKeys.all`.
 */
export function useAttentionCounts({ org, workspace }: { org?: string; workspace?: string }, enabled = true) {
  return useQuery({
    queryKey: attentionKeys.counts(org, workspace),
    enabled,
    retry: false,
    staleTime: 30_000,
    refetchOnWindowFocus: true,
    refetchInterval: ATTENTION_REFETCH_MS,
    queryFn: () =>
      unwrap(
        api.GET("/v1/attention/counts", {
          params: { query: { ...(org ? { org } : {}), ...(workspace ? { workspace } : {}) } },
        }),
      ),
  });
}

/** The badge refetches on window focus and at this interval (FIX-167, D-370). */
export const ATTENTION_REFETCH_MS = 5 * 60_000;

/**
 * `GET /v1/admin/attention` (C-1022): what needs an instance admin. `enabled` is false for everyone else
 * (the route answers 403). A failed fetch is not retried and not shown: the badge simply hides.
 */
export function useAdminAttention(enabled = true) {
  return useQuery({
    queryKey: attentionKey,
    queryFn: () => unwrap(api.GET("/v1/admin/attention")),
    enabled,
    retry: false,
    staleTime: 30_000,
    refetchOnWindowFocus: true,
    refetchInterval: ATTENTION_REFETCH_MS,
  });
}

/**
 * (P5-REFINE-18 #18) The count drops without a reload: any mutation that succeeds may have resolved an item
 * (an approval, a decision, a connected platform), so the attention query is refetched, once per success. Only an
 * active query refetches, so a person without the badge fetches nothing.
 */
export function useRefreshAttentionOnMutation(enabled: boolean): void {
  const qc = useQueryClient();
  useEffect(() => {
    if (!enabled) return;
    return qc.getMutationCache().subscribe((e) => {
      if (e.type === "updated" && e.action.type === "success") void qc.invalidateQueries({ queryKey: attentionKey });
    });
  }, [qc, enabled]);
}
