"use client";

import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect } from "react";

import { api } from "./client";
import type { components } from "./schema";
import { unwrap } from "./unwrap";

type SchemaItem = components["schemas"]["AttentionItem"];
// FIX-282 contract, typed here until `schema.d.ts` is regenerated after it merges.
export type AttentionItem = Omit<SchemaItem, "kind"> & { kind: SchemaItem["kind"] | "library_update" | "org_account_broken" };
export type AttentionList = Omit<components["schemas"]["AttentionList"], "items"> & { items: AttentionItem[] };
export type AttentionCounts = components["schemas"]["AttentionCounts"];

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
 * (never an error). Refetched on window focus and every 30 seconds; deciding, declining or resolving an item
 * invalidates `attentionKeys.all`.
 */
export function useAttentionCounts(
  { org, workspace }: { org?: string; workspace?: string },
  enabled = true,
  { intervalMs = ATTENTION_REFETCH_MS, staleTime = 30_000 }: { intervalMs?: number; staleTime?: number } = {},
) {
  return useQuery({
    queryKey: attentionKeys.counts(org, workspace),
    enabled,
    retry: false,
    staleTime,
    refetchOnWindowFocus: true,
    refetchInterval: () => (typeof document === "undefined" || document.visibilityState === "visible" ? intervalMs : false),
    queryFn: () =>
      unwrap(
        api.GET("/v1/attention/counts", {
          params: { query: { ...(org ? { org } : {}), ...(workspace ? { workspace } : {}) } },
        }),
      ),
  });
}

/** The workspace rail's numbers are read again this often (the pending approvals' own poll, P5-REFINE-16 G1). */
export const WORKSPACE_COUNTS_MS = 30_000;

/**
 * (P5-REFINE-16 item 19, G1/G2/G6) The one hook behind every workspace count badge: Approvals, Overview, the phone drawer
 * and the menu dot all read it, so they show the same number and ask once (one query key). `count` is the whole level
 * (Overview), `topics` the part per page (`approvals`). Both are 0 for someone who cannot act there (the scope comes back
 * `null`), while loading and when the read fails: a badge never shows an error.
 */
export function useWorkspaceCounts(wsId: string | undefined): { count: number; topics: Record<string, number> } {
  const q = useAttentionCounts({ workspace: wsId }, !!wsId, { intervalMs: WORKSPACE_COUNTS_MS, staleTime: 0 });
  const w = wsId && !q.isError ? q.data?.workspace : null;
  return { count: w?.count ?? 0, topics: w?.topics ?? {} };
}

/** The badge refetches on window focus and at this interval, like the workspace ones (P5-REFINE-20 G4, was 5 minutes in FIX-167). */
export const ATTENTION_REFETCH_MS = 30_000;

/**
 * (P5-REFINE-18 #18, P5-REFINE-17 #17) The count drops without a reload: any mutation that succeeds may have resolved an
 * item (an approval, a decision, a connected platform), so every attention query (feeds and counts) is refetched, once per
 * success. Only an active query refetches, so a person without a badge fetches nothing.
 */
export function useRefreshAttentionOnMutation(enabled: boolean): void {
  const qc = useQueryClient();
  useEffect(() => {
    if (!enabled) return;
    return qc.getMutationCache().subscribe((e) => {
      if (e.type === "updated" && e.action.type === "success") {
        void qc.invalidateQueries({ queryKey: attentionKeys.all });
      }
    });
  }, [qc, enabled]);
}

/** One level's number for a rail: the total and the open items per topic (`topics`, C-1082). */
export interface LevelBadge {
  count: number;
  topics: Record<string, number>;
}

/**
 * (P5-REFINE-18 #18, C-1082) What waits in Administration for an instance admin: the total for the Overview entry and the
 * sidebar link, and a part per topic (`apps`, `chat-platforms`, `people`). `null` for everyone else, while loading and when
 * the read failed: a badge never shows an error.
 */
export function useAdminBadge(enabled = true): LevelBadge | null {
  const q = useAttentionCounts({}, enabled);
  if (!enabled || q.isError) return null;
  return q.data?.administration ?? null;
}

/**
 * (P5-REFINE-17 #17, C-1082) What waits in one organization for its owners and admins (requests for apps, chat platforms
 * and AI keys): the total for the Manage entry and the Overview, and a part per topic. `null` for everyone else.
 */
export function useOrgBadge(orgId: string | undefined, enabled = true): LevelBadge | null {
  const on = enabled && !!orgId;
  const q = useAttentionCounts({ org: orgId }, on);
  if (!on || q.isError) return null;
  return q.data?.organization ?? null;
}
