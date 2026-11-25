"use client";

// (FIX-236, C-1164) The emergency read access routes: open (an org owner, from the manage page), end
// (the reader banner's "End now"), and the two history lists (the workspace's People page and the
// organization's audit topic). The rows are metadata only — who, why, from, until, how it ended.

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import { api } from "./client";
import { retryUnlessClientError } from "./errors";
import { fetchAllPages, PAGE_LIMIT } from "./paging";
import type { components } from "./schema";
import { unwrap } from "./unwrap";

export type EmergencyAccess = components["schemas"]["EmergencyAccess"];
export type EmergencyAccessRow = components["schemas"]["Page_EmergencyAccess"]["items"][number];

export const emergencyKeys = {
  wsList: (wsId: string) => ["workspaces", wsId, "emergency-access"] as const,
  orgList: (orgId: string) => ["orgs", orgId, "emergency-access"] as const,
};

/** `GET /v1/workspaces/{ws}/emergency-access`: the workspace's rows, newest first. */
export function useWorkspaceEmergencyAccess(wsId: string | undefined, enabled = true) {
  return useQuery({
    queryKey: emergencyKeys.wsList(wsId ?? ""),
    queryFn: () =>
      fetchAllPages((cursor) =>
        unwrap(
          api.GET("/v1/workspaces/{ws}/emergency-access", {
            params: { path: { ws: wsId! }, query: { limit: PAGE_LIMIT, cursor } },
          }),
        ),
      ),
    enabled: !!wsId && enabled,
    retry: retryUnlessClientError,
  });
}

/** `GET /v1/orgs/{org}/emergency-access`: the organization's rows, newest first (org owners and admins). */
export function useOrgEmergencyAccess(orgId: string | undefined, enabled = true) {
  return useQuery({
    queryKey: emergencyKeys.orgList(orgId ?? ""),
    queryFn: () =>
      fetchAllPages((cursor) =>
        unwrap(
          api.GET("/v1/orgs/{org}/emergency-access", {
            params: { path: { org: orgId! }, query: { limit: PAGE_LIMIT, cursor } },
          }),
        ),
      ),
    enabled: !!orgId && enabled,
    retry: retryUnlessClientError,
  });
}

/** `POST /v1/workspaces/{ws}/emergency-access` (C-1164): 201 with the row; 403/409/422 surface as errors. */
export function useOpenEmergencyAccess(wsId: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (body: components["schemas"]["OpenEmergencyAccessRequest"]) =>
      unwrap(api.POST("/v1/workspaces/{ws}/emergency-access", { params: { path: { ws: wsId } }, body })),
    onSuccess: () => {
      // The workspace answer carries `audience.emergency` and `emergency_reader`.
      void qc.invalidateQueries({ queryKey: ["workspaces"] });
      void qc.invalidateQueries({ queryKey: ["resolve"] });
    },
  });
}

/** `POST /v1/emergency-access/{id}/end`: idempotent; the row answers with its new `ended_at`. */
export function useEndEmergencyAccess() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (id: string) => unwrap(api.POST("/v1/emergency-access/{id}/end", { params: { path: { id } } })),
    onSuccess: () => {
      // The workspace and resolve answers carry `emergency_reader`; the history lists start with the
      // same "workspaces" / "orgs" keys, so the two invalidations cover them too.
      void qc.invalidateQueries({ queryKey: ["workspaces"] });
      void qc.invalidateQueries({ queryKey: ["orgs"] });
      void qc.invalidateQueries({ queryKey: ["resolve"] });
    },
  });
}
