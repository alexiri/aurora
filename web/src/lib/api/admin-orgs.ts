"use client";

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import { api } from "./client";
import { fetchAllPages, PAGE_LIMIT } from "./paging";
import type { components } from "./schema";
import { unwrap } from "./unwrap";

export type AdminOrgWorkspace = components["schemas"]["AdminWorkspace"];

/** One organization's operator row: counts, no content (P5-ADMDEL, C-733). */
export function useAdminOrg(orgId: string) {
  return useQuery({
    queryKey: ["admin", "org", orgId] as const,
    queryFn: () => unwrap(api.GET("/v1/admin/orgs/{org}", { params: { path: { org: orgId } } })),
  });
}

/** The organization's workspaces, metadata only. */
export function useAdminOrgWorkspaces(orgId: string) {
  return useQuery({
    queryKey: ["admin", "org", orgId, "workspaces"] as const,
    queryFn: () =>
      fetchAllPages((cursor) =>
        unwrap(api.GET("/v1/admin/orgs/{org}/workspaces", { params: { path: { org: orgId }, query: { limit: PAGE_LIMIT, cursor } } })),
      ),
  });
}

/** Deleting reshapes the lists and counts everywhere an admin looks: refetch the admin queries. */
function useRefreshAdmin() {
  const qc = useQueryClient();
  return () => qc.invalidateQueries({ queryKey: ["admin"] });
}

/** `DELETE /v1/admin/orgs/{org}` with the typed slug (no re-authentication, FIX-157, D-317); an API error surfaces as the mutation error. */
export function useAdminDeleteOrg() {
  const refresh = useRefreshAdmin();
  return useMutation({
    mutationFn: (v: { orgId: string; confirm: string }) =>
      unwrap(api.DELETE("/v1/admin/orgs/{org}", { params: { path: { org: v.orgId } }, body: { confirm: v.confirm } })),
    onSuccess: refresh,
  });
}

/** `DELETE /v1/admin/workspaces/{ws}` with the typed slug. */
export function useAdminDeleteWorkspace() {
  const refresh = useRefreshAdmin();
  return useMutation({
    mutationFn: (v: { wsId: string; confirm: string }) =>
      unwrap(api.DELETE("/v1/admin/workspaces/{ws}", { params: { path: { ws: v.wsId } }, body: { confirm: v.confirm } })),
    onSuccess: refresh,
  });
}
