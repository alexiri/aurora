"use client";

import { useInfiniteQuery, useMutation, useQuery, useQueryClient, type QueryClient } from "@tanstack/react-query";
import { useEffect, useState } from "react";

import { api, ApiError } from "./client";
import { useWorkspace } from "./queries";
import { fetchAllPages, PAGE_LIMIT } from "./paging";
import { setRateNotice } from "./rate-limit";
import type { components } from "./schema";
import { unwrap } from "./unwrap";

type S = components["schemas"];
export type Approval = S["Approval"];
export type ApprovalEvent = S["ApprovalEvent"];
export type RequestLogEntry = S["RequestLogEntry"];
export type Decision = S["Decision"];

const retry = (count: number, err: unknown) =>
  !(err instanceof ApiError && err.status !== undefined && err.status >= 400 && err.status < 500) &&
  count < 1;

export const approvalKeys = {
  all: (ws: string) => ["approvals", ws] as const,
  pending: (ws: string) => ["approvals", ws, "pending"] as const,
  history: (ws: string, status: string) => ["approvals", ws, "history", status] as const,
  activity: (ws: string, agent: string, decision: string, host: string) =>
    ["activity", ws, agent, decision, host] as const,
};

export interface PendingData {
  items: Approval[];
  /** More pending approvals exist than the pages read (the ceiling of `fetchAllPages`). */
  truncated?: boolean;
}

/** Pure reducer: apply an SSE event to the pending list. */
export function applyApprovalEvent(data: PendingData | undefined, evt: ApprovalEvent): PendingData | undefined {
  if (!data) return data;
  const { approval } = evt;
  const rest = data.items.filter((a) => a.id !== approval.id);
  if (evt.event === "created" && approval.status === "pending") {
    return { ...data, items: [...rest, approval].sort((a, b) => a.created_at.localeCompare(b.created_at)) };
  }
  return { ...data, items: rest };
}

export function parseApprovalEvent(raw: string): ApprovalEvent | null {
  try {
    const v = JSON.parse(raw) as ApprovalEvent;
    if (v && typeof v.event === "string" && v.approval && typeof v.approval.id === "string") return v;
  } catch {
    /* ignore malformed frames */
  }
  return null;
}

async function fetchPending(wsId: string): Promise<PendingData> {
  // Every pending approval is decidable from the dashboard, so every page is read (FIX-117).
  const all = await fetchAllPages((cursor) =>
    unwrap(
      api.GET("/v1/workspaces/{ws}/approvals", {
        params: { path: { ws: wsId }, query: { status: "pending", limit: PAGE_LIMIT, cursor } },
      }),
    ),
  );
  return { items: [...all.items].sort((a, b) => a.created_at.localeCompare(b.created_at)), truncated: all.truncated };
}

/** Phase 2, RFE-012 (D-55): `can_approve` = effective workspace role owner or approver. */
export function useCanApprove(wsId: string | undefined): boolean {
  const role = useWorkspace(wsId).data?.my_role;
  return role === "owner" || role === "approver";
}

export function usePendingApprovals(wsId: string, enabled = true) {
  return useQuery({ queryKey: approvalKeys.pending(wsId), queryFn: () => fetchPending(wsId), enabled: !!wsId && enabled, retry });
}

/** Subscribes to the approvals SSE stream, keeping the pending cache live (C-10: refetch on reconnect). */
export function useApprovalsStream(wsId: string): { live: boolean } {
  const qc = useQueryClient();
  const [live, setLive] = useState(false);
  useEffect(() => {
    if (typeof EventSource === "undefined") return;
    const es = new EventSource("/v1/approvals/stream");
    let opened = false;
    es.onopen = () => {
      setLive(true);
      // No SSE ids: after any reconnect, refetch to catch up on missed events.
      if (opened) void qc.invalidateQueries({ queryKey: approvalKeys.all(wsId) });
      opened = true;
    };
    es.onerror = () => {
      setLive(false);
      // A refused connection (e.g. 429 too_many_streams) is CLOSED, not retried by the browser.
      if (es.readyState === EventSource.CLOSED) setRateNotice({ kind: "streams" });
    };
    // (FIX-099, C-10) The server's `resync` frame: notifications may have been missed (a lagged receiver, a bus
    // reconnect) — refetch rather than keep decided or expired approvals as pending.
    es.addEventListener("resync", () => void qc.invalidateQueries({ queryKey: approvalKeys.all(wsId) }));
    es.addEventListener("approval", (e) => {
      const evt = parseApprovalEvent((e as MessageEvent<string>).data);
      if (!evt || evt.approval.workspace_id !== wsId) return;
      qc.setQueryData<PendingData>(approvalKeys.pending(wsId), (d) => applyApprovalEvent(d, evt));
      if (evt.event !== "created") void qc.invalidateQueries({ queryKey: [...approvalKeys.all(wsId), "history"] });
    });
    return () => es.close();
  }, [qc, wsId]);
  return { live };
}

function removeFromPending(qc: QueryClient, wsId: string, ids: (string | undefined)[]) {
  qc.setQueryData<PendingData>(approvalKeys.pending(wsId), (d) =>
    d ? { ...d, items: d.items.filter((a) => !ids.includes(a.id)) } : d,
  );
}

export function isAlreadyDecided(err: unknown): boolean {
  return err instanceof ApiError && (err.code === "already_decided" || err.status === 409);
}

export function useDecide(wsId: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({ id, decision }: { id: string; decision: Decision }) =>
      unwrap(api.POST("/v1/approvals/{id}/decision", { params: { path: { id } }, body: { decision } })),
    onSettled: (_d, err, vars) => {
      // On 409 someone else decided; either way it is no longer pending for us.
      if (!err || isAlreadyDecided(err)) removeFromPending(qc, wsId, [vars.id]);
      void qc.invalidateQueries({ queryKey: approvalKeys.all(wsId) });
    },
  });
}

export function useDecideBatch(wsId: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({ batchId, agentId, decision }: { batchId: string; agentId: string; decision: Decision }) =>
      unwrap(
        api.POST("/v1/approvals/batches/{batch_id}/decision", {
          params: { path: { batch_id: batchId } },
          body: { agent_id: agentId, decision },
        }),
      ),
    onSettled: () => void qc.invalidateQueries({ queryKey: approvalKeys.all(wsId) }),
  });
}

export function useApprovalHistory(wsId: string, status: string) {
  return useInfiniteQuery({
    queryKey: approvalKeys.history(wsId, status),
    initialPageParam: undefined as string | undefined,
    queryFn: ({ pageParam }) =>
      unwrap(
        api.GET("/v1/workspaces/{ws}/approvals", {
          params: { path: { ws: wsId }, query: { status: status || undefined, limit: 25, cursor: pageParam } },
        }),
      ),
    getNextPageParam: (last) => last.next_cursor ?? undefined,
    retry,
  });
}

export function useActivity(wsId: string, f: { agent: string; decision: string; host: string }) {
  return useInfiniteQuery({
    queryKey: approvalKeys.activity(wsId, f.agent, f.decision, f.host),
    initialPageParam: undefined as string | undefined,
    queryFn: ({ pageParam }) =>
      unwrap(
        api.GET("/v1/workspaces/{ws}/activity", {
          params: {
            path: { ws: wsId },
            query: {
              limit: 50,
              cursor: pageParam,
              agent_id: f.agent || undefined,
              decision: f.decision || undefined,
              host: f.host || undefined,
            },
          },
        }),
      ),
    getNextPageParam: (last) => last.next_cursor ?? undefined,
    retry,
  });
}

/** Seconds-granularity ticking clock for countdowns. */
export function useNow(intervalMs = 1000): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const t = setInterval(() => setNow(Date.now()), intervalMs);
    return () => clearInterval(t);
  }, [intervalMs]);
  return now;
}

export function countdown(deadline: string, now: number): string {
  const s = Math.floor((new Date(deadline).getTime() - now) / 1000);
  if (s <= 0) return "expired";
  const m = Math.floor(s / 60);
  return m >= 60 ? `${Math.floor(m / 60)}h ${m % 60}m` : `${m}m ${String(s % 60).padStart(2, "0")}s`;
}
