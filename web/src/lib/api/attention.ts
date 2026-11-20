"use client";

import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect } from "react";

import { api } from "./client";
import type { components } from "./schema";
import { unwrap } from "./unwrap";

export type AttentionItem = components["schemas"]["AttentionItem"];
export type AttentionList = components["schemas"]["AttentionList"];

export const attentionKey = ["admin", "attention"] as const;

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
