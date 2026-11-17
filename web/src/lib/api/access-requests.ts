"use client";

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import { api } from "./client";
import type { components } from "./schema";
import { unwrap } from "./unwrap";

type S = components["schemas"];
export type AccessRequest = S["AccessRequest"];

export const accessRequestKeys = {
  open: (ws: string) => ["access-requests", ws, "open"] as const,
};

/** (P5-UX-17, C-984) Agents' open requests for access in a workspace: the banner above the Accounts list. */
export function useAccessRequests(wsId: string | undefined, enabled = true) {
  return useQuery({
    queryKey: accessRequestKeys.open(wsId ?? ""),
    enabled: enabled && !!wsId,
    refetchInterval: 30_000,
    queryFn: () => unwrap(api.GET("/v1/workspaces/{ws}/access-requests", { params: { path: { ws: wsId! }, query: { status: "open" } } })),
  });
}

/** "Not now": folds the request's card away (`can_own`). */
export function useDismissAccessRequest(wsId: string | undefined) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (id: string) => unwrap(api.POST("/v1/access-requests/{id}/dismiss", { params: { path: { id } } })),
    onSuccess: () => void qc.invalidateQueries({ queryKey: accessRequestKeys.open(wsId ?? "") }),
  });
}
