"use client";

import { useQuery } from "@tanstack/react-query";

import { attentionKeys } from "./attention";
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

/**
 * (P5-REFINE-8, C-1081) An organization's requests closed in the last 30 days (`status=history`), newest first, every
 * level. Owners and admins only.
 */
export function useOrgSetupHistory(orgId: string | undefined, enabled = true) {
  return useQuery({
    queryKey: [...setupRequestKeys.org(orgId ?? ""), "history"] as const,
    enabled: enabled && !!orgId,
    retry: false,
    queryFn: async () =>
      (await unwrap(api.GET("/v1/orgs/{org}/app-setup-requests", { params: { path: { org: orgId! }, query: { status: "history" } } })))
        .items,
  });
}

/** An app ask: the app id alone, or (C-1081) with the person's optional reason (at most 300 characters). */
export type AskSetupArgs = string | { app: string; reason?: string };

/** "Ask an admin": asks the organization's owners and admins to make the app ready. Idempotent per person. */
export function useAskSetup(workspaceId: string) {
  return useInvalidatingMutation(
    (a: AskSetupArgs) => {
      const { app, reason } = typeof a === "string" ? { app: a, reason: undefined } : a;
      return unwrap(
        api.POST("/v1/workspaces/{ws}/apps/{app}/setup-request", {
          params: { path: { ws: workspaceId, app } },
          ...(reason ? { body: { reason } } : {}),
        }),
      );
    },
    ["apps"],
    ["workspace-overview", workspaceId],
    myRequestKeys.all(workspaceId),
    attentionKeys.all,
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

/** (C-1081) The caller's own list: `open` (default) or `history` (ended in the last 30 days). */
export type MyRequestsStatus = "open" | "history";

export const myRequestKeys = {
  all: (workspaceId: string) => ["setup-requests", "mine", workspaceId] as const,
  mine: (workspaceId: string, status: MyRequestsStatus = "open") =>
    status === "open" ? myRequestKeys.all(workspaceId) : ([...myRequestKeys.all(workspaceId), status] as const),
};

/**
 * (P5-APIGAP-2, C-1047) The caller's own open requests of every subject in this workspace's organization. (P5-REFINE-8,
 * C-1081) `status: "history"`: theirs that ended in the last 30 days, with the decline message and the role (never the
 * admin's name).
 */
export function useMyRequests(workspaceId: string | undefined, status: MyRequestsStatus = "open") {
  return useQuery({
    queryKey: myRequestKeys.mine(workspaceId ?? "", status),
    enabled: !!workspaceId,
    retry: false,
    queryFn: async () =>
      (await unwrap(api.GET("/v1/workspaces/{ws}/requests", { params: { path: { ws: workspaceId! }, query: { status } } }))).items,
  });
}

/** "Ask an admin" for a chat platform or an AI key, with an optional reason (C-1081). 409 `already_available` when it exists. */
export function useAskRequest(workspaceId: string) {
  return useInvalidatingMutation(
    (a: { subject: RequestSubject; connectorType?: string; reason?: string }) =>
      unwrap(
        api.POST("/v1/workspaces/{ws}/requests", {
          params: { path: { ws: workspaceId } },
          body: { subject: a.subject, connector_type: a.connectorType, ...(a.reason ? { reason: a.reason } : {}) },
        }),
      ),
    myRequestKeys.all(workspaceId),
    ["workspace-overview", workspaceId],
    attentionKeys.all,
  );
}

/** Withdraws the person's ask by id ("Undo"). */
export function useWithdrawRequest(workspaceId: string) {
  return useInvalidatingMutation(
    async (id: string) => {
      const { error, response } = await api.DELETE("/v1/workspaces/{ws}/requests/{id}", { params: { path: { ws: workspaceId, id } } });
      if (error !== undefined || !response.ok) throw ApiError.fromBody(error, response.status);
    },
    myRequestKeys.all(workspaceId),
    ["workspace-overview", workspaceId],
    attentionKeys.all,
  );
}

/** A decline: the request id alone, or (C-1081) with an optional message the askers read (at most 300 characters). */
export type DeclineArgs = string | { id: string; message?: string };

function declineArgs(a: DeclineArgs): { id: string; message?: string } {
  return typeof a === "string" ? { id: a } : a;
}

/**
 * "Decline": an owner or admin declines a request of the organization's level; each asker is told once, by role
 * ("An admin isn't setting up …"), with the message when there is one.
 */
export function useDeclineSetup(orgId: string) {
  return useInvalidatingMutation(
    (a: DeclineArgs) => {
      const { id, message } = declineArgs(a);
      return unwrap(
        api.POST("/v1/orgs/{org}/app-setup-requests/{id}/decline", {
          params: { path: { org: orgId, id } },
          ...(message ? { body: { message } } : {}),
        }),
      );
    },
    setupRequestKeys.org(orgId),
    ["apps"],
    attentionKeys.all,
  );
}

export const adminRequestKeys = {
  list: (status: MyRequestsStatus) => ["admin", "requests", status] as const,
};

/** (P5-REFINE-8, C-1081) The requests only an instance admin can resolve (`open`) or that closed in 30 days (`history`). */
export function useAdminRequests(status: MyRequestsStatus = "open", enabled = true) {
  return useQuery({
    queryKey: adminRequestKeys.list(status),
    enabled,
    retry: false,
    queryFn: async () => (await unwrap(api.GET("/v1/admin/requests", { params: { query: { status } } }))).items,
  });
}

/** (C-1081) An instance admin declines a request of the Administration level, with an optional message. */
export function useAdminDeclineRequest() {
  return useInvalidatingMutation(
    (a: DeclineArgs) => {
      const { id, message } = declineArgs(a);
      return unwrap(api.POST("/v1/admin/requests/{id}/decline", { params: { path: { id } }, ...(message ? { body: { message } } : {}) }));
    },
    // Administration's requests and its attention page (`["admin", "attention"]`).
    ["admin"],
    attentionKeys.all,
  );
}
