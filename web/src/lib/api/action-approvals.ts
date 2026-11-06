"use client";

import { useInfiniteQuery, useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect, useState } from "react";

import { api, ApiError } from "./client";
import type { components } from "./schema";
import { unwrap } from "./unwrap";

export type ActionApproval = components["schemas"]["ActionApproval"];
export type ActionDecision = components["schemas"]["Decision"];

export const actionApprovalKeys = {
  all: (ws: string) => ["action-approvals", ws] as const,
  pending: (ws: string) => ["action-approvals", ws, "pending"] as const,
  history: (ws: string, status: string) => ["action-approvals", ws, "history", status] as const,
};

/** True while the page is visible; polling pauses in background tabs. */
function usePageVisible(): boolean {
  const [visible, setVisible] = useState(() => typeof document === "undefined" || document.visibilityState !== "hidden");
  useEffect(() => {
    const on = () => setVisible(document.visibilityState !== "hidden");
    document.addEventListener("visibilitychange", on);
    return () => document.removeEventListener("visibilitychange", on);
  }, []);
  return visible;
}

export const ACTION_POLL_MS = 10_000;

/** The approvals SSE stream carries gateway holds only, so action approvals are polled while visible. */
export function usePendingActionApprovals(ws: string) {
  const visible = usePageVisible();
  return useQuery({
    queryKey: actionApprovalKeys.pending(ws),
    queryFn: () =>
      unwrap(api.GET("/v1/workspaces/{ws}/action-approvals", { params: { path: { ws }, query: { status: "pending", limit: 100 } } })),
    enabled: !!ws,
    refetchInterval: visible ? ACTION_POLL_MS : false,
    retry: false,
  });
}

export function useActionApprovalHistory(ws: string, status: string) {
  return useInfiniteQuery({
    queryKey: actionApprovalKeys.history(ws, status),
    initialPageParam: undefined as string | undefined,
    queryFn: ({ pageParam }) =>
      unwrap(
        api.GET("/v1/workspaces/{ws}/action-approvals", {
          params: { path: { ws }, query: { status: status || undefined, limit: 25, cursor: pageParam } },
        }),
      ),
    getNextPageParam: (last) => last.next_cursor ?? undefined,
    retry: false,
  });
}

export function useDecideAction(ws: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({ id, decision }: { id: string; decision: ActionDecision }) =>
      unwrap(api.POST("/v1/action-approvals/{id}/decision", { params: { path: { id } }, body: { decision } })),
    // 409 `already_decided`: someone else got there first; refresh instead of leaving a stale card.
    onSettled: () => void qc.invalidateQueries({ queryKey: actionApprovalKeys.all(ws) }),
  });
}

export function isAlreadyDecided(err: unknown): boolean {
  return err instanceof ApiError && (err.code === "already_decided" || err.status === 409);
}
