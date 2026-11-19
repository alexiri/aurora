"use client";

import { useQuery } from "@tanstack/react-query";

import { ApiError, api } from "./client";
import { useInvalidatingMutation } from "./mutations";
import type { components } from "./schema";
import { unwrap } from "./unwrap";

type S = components["schemas"];
export type AppSetupRequest = S["AppSetupRequest"];

export const setupRequestKeys = {
  org: (orgId: string) => ["app-setup-requests", orgId] as const,
};

/**
 * (P5-POLISH-4, D-386, C-1030) An organization's open app setup requests: owners and admins only (anyone else gets
 * a 404, so callers enable it for them alone). Feeds the Overview's Needs you.
 */
export function useOrgSetupRequests(orgId: string | undefined, enabled = true) {
  return useQuery({
    queryKey: setupRequestKeys.org(orgId ?? ""),
    enabled: enabled && !!orgId,
    refetchInterval: 60_000,
    retry: false,
    queryFn: async () =>
      (await unwrap(api.GET("/v1/orgs/{org}/app-setup-requests", { params: { path: { org: orgId! }, query: { status: "open" } } }))).items,
  });
}

/** "Ask <Name>": asks the organization's owners and admins to make the app ready. Idempotent per person. */
export function useAskSetup(workspaceId: string) {
  return useInvalidatingMutation(
    (app: string) => unwrap(api.POST("/v1/workspaces/{ws}/apps/{app}/setup-request", { params: { path: { ws: workspaceId, app } } })),
    ["apps"],
    ["workspace-overview", workspaceId],
  );
}

/** Withdraws the person's ask ("Undo"). */
export function useWithdrawSetup(workspaceId: string) {
  return useInvalidatingMutation(
    async (app: string) => {
      const { error, response } = await api.DELETE("/v1/workspaces/{ws}/apps/{app}/setup-request", { params: { path: { ws: workspaceId, app } } });
      if (error !== undefined || !response.ok) throw ApiError.fromBody(error, response.status);
    },
    ["apps"],
    ["workspace-overview", workspaceId],
  );
}

/** What a setup request asks for (C-1047): `app` has its own routes and hooks above. */
export type RequestSubject = "chat_platform" | "ai_key";

export const myRequestKeys = {
  mine: (workspaceId: string) => ["setup-requests", "mine", workspaceId] as const,
};

/** (P5-APIGAP-2, C-1047) The caller's own open requests of every subject in this workspace's organization. */
export function useMyRequests(workspaceId: string | undefined) {
  return useQuery({
    queryKey: myRequestKeys.mine(workspaceId ?? ""),
    enabled: !!workspaceId,
    retry: false,
    queryFn: async () => (await unwrap(api.GET("/v1/workspaces/{ws}/requests", { params: { path: { ws: workspaceId! } } }))).items,
  });
}

/** "Ask <Name>" for a chat platform or an AI key. Idempotent per person; 409 `already_available` when it exists. */
export function useAskRequest(workspaceId: string) {
  return useInvalidatingMutation(
    (a: { subject: RequestSubject; connectorType?: string }) =>
      unwrap(
        api.POST("/v1/workspaces/{ws}/requests", {
          params: { path: { ws: workspaceId } },
          body: { subject: a.subject, connector_type: a.connectorType },
        }),
      ),
    myRequestKeys.mine(workspaceId),
    ["workspace-overview", workspaceId],
  );
}

/** Withdraws the person's ask by id ("Undo"). */
export function useWithdrawRequest(workspaceId: string) {
  return useInvalidatingMutation(
    async (id: string) => {
      const { error, response } = await api.DELETE("/v1/workspaces/{ws}/requests/{id}", { params: { path: { ws: workspaceId, id } } });
      if (error !== undefined || !response.ok) throw ApiError.fromBody(error, response.status);
    },
    myRequestKeys.mine(workspaceId),
    ["workspace-overview", workspaceId],
  );
}

/** "Not now": an owner or admin declines; each asker is told once. */
export function useDeclineSetup(orgId: string) {
  return useInvalidatingMutation(
    (id: string) => unwrap(api.POST("/v1/orgs/{org}/app-setup-requests/{id}/decline", { params: { path: { org: orgId, id } } })),
    setupRequestKeys.org(orgId),
    ["apps"],
  );
}
