"use client";

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import { api } from "./client";
import { useOrg } from "./queries";
import { fetchAllPages, PAGE_LIMIT } from "./paging";
import type { components } from "./schema";
import { unwrap } from "./unwrap";

export type Frozen = components["schemas"]["Frozen"];
export type FreezeResult = components["schemas"]["FreezeResult"];
export type AdminOrg = components["schemas"]["AdminOrg"];

/** Rank of a freeze authority: `workspace_owner` < `org_admin` < `instance_admin`. */
export function authorityRank(a: string): number {
  switch (a) {
    case "workspace_owner":
      return 1;
    case "org_admin":
      return 2;
    case "instance_admin":
      return 3;
    default:
      return Number.MAX_SAFE_INTEGER;
  }
}

/** The authority the signed-in person acts with on an org's workspaces and agents (RFE-006). */
export function useFreezeAuthority(orgId: string | undefined): number {
  const org = useOrg(orgId);
  const role = org.data?.my_role;
  return role === "owner" || role === "admin" ? authorityRank("org_admin") : authorityRank("workspace_owner");
}

/** A freeze changes banners and actions everywhere; refetch it all. */
function useRefreshAll() {
  const qc = useQueryClient();
  return () => qc.invalidateQueries();
}

export function useFreezeAgent(agentId: string) {
  const refresh = useRefreshAll();
  return {
    freeze: useMutation({
      mutationFn: (reason: string) =>
        unwrap(api.POST("/v1/agents/{agent}/freeze", { params: { path: { agent: agentId } }, body: { reason } })),
      onSuccess: refresh,
    }),
    unfreeze: useMutation({
      mutationFn: () => unwrap(api.POST("/v1/agents/{agent}/unfreeze", { params: { path: { agent: agentId } } })),
      onSuccess: refresh,
    }),
  };
}

export function useFreezeWorkspace(wsId: string) {
  const refresh = useRefreshAll();
  return {
    freeze: useMutation({
      mutationFn: (reason: string) =>
        unwrap(api.POST("/v1/workspaces/{ws}/freeze", { params: { path: { ws: wsId } }, body: { reason } })),
      onSuccess: refresh,
    }),
    unfreeze: useMutation({
      mutationFn: () => unwrap(api.POST("/v1/workspaces/{ws}/unfreeze", { params: { path: { ws: wsId } } })),
      onSuccess: refresh,
    }),
  };
}

export function useFreezeOrg(orgId: string) {
  const refresh = useRefreshAll();
  return {
    freeze: useMutation({
      mutationFn: (reason: string) =>
        unwrap(api.POST("/v1/orgs/{org}/freeze", { params: { path: { org: orgId } }, body: { reason } })),
      onSuccess: refresh,
    }),
    unfreeze: useMutation({
      mutationFn: () => unwrap(api.POST("/v1/orgs/{org}/unfreeze", { params: { path: { org: orgId } } })),
      onSuccess: refresh,
    }),
  };
}

/** Instance administration (RFE-006): organizations and their freeze state. */
export function useAdminOrgs(frozenOnly: boolean) {
  return useQuery({
    queryKey: ["admin", "orgs", frozenOnly] as const,
    // Every page: an operator must be able to freeze an org past the first page (FIX-117).
    queryFn: () =>
      fetchAllPages((cursor) =>
        unwrap(api.GET("/v1/admin/orgs", { params: { query: { frozen: frozenOnly || undefined, limit: PAGE_LIMIT, cursor } } })),
      ),
  });
}

export function useAdminFreezeOrg(orgId: string) {
  const refresh = useRefreshAll();
  return {
    freeze: useMutation({
      mutationFn: (reason: string) =>
        unwrap(api.POST("/v1/admin/orgs/{org}/freeze", { params: { path: { org: orgId } }, body: { reason } })),
      onSuccess: refresh,
    }),
    unfreeze: useMutation({
      mutationFn: () => unwrap(api.POST("/v1/admin/orgs/{org}/unfreeze", { params: { path: { org: orgId } } })),
      onSuccess: refresh,
    }),
  };
}
