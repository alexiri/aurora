"use client";

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import { accountKeys } from "./account";
import { api } from "./client";
import type { components } from "./schema";
import { unwrap } from "./unwrap";
import { ApiError } from "./client";

type S = components["schemas"];
export type AddByEmailResponse = S["AddByEmailResponse"];
export type PendingAccess = S["PendingAccess"];

export const pendingKeys = {
  list: (ws: string) => ["workspaces", ws, "access", "pending"] as const,
};

/** (C-920) The people waiting for their first sign-in (workspace owners). */
export function usePendingAccess(wsId: string, enabled = true) {
  return useQuery({
    queryKey: pendingKeys.list(wsId),
    enabled,
    queryFn: () => unwrap(api.GET("/v1/workspaces/{ws}/access/pending", { params: { path: { ws: wsId } } })),
    retry: false,
  });
}

/** (C-919) Add one person by e-mail: bound at once for an organization member, else waiting for the first sign-in. */
export function useAddByEmail(wsId: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (v: { email: string; role: "owner" | "approver" | "member" }) =>
      unwrap(api.POST("/v1/workspaces/{ws}/access/by-email", { params: { path: { ws: wsId } }, body: v })),
    onSuccess: () => {
      void qc.invalidateQueries({ queryKey: pendingKeys.list(wsId) });
      void qc.invalidateQueries({ queryKey: accountKeys.access(wsId) });
    },
  });
}

/** (C-920) Remove one waiting person. */
export function useRemovePending(wsId: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: async (id: string) => {
      const r = await api.DELETE("/v1/workspaces/{ws}/access/pending/{id}", { params: { path: { ws: wsId, id } } });
      if (r.error !== undefined || !r.response.ok) throw ApiError.fromBody(r.error, r.response.status);
    },
    onSuccess: () => void qc.invalidateQueries({ queryKey: pendingKeys.list(wsId) }),
  });
}
