"use client";

import { useInfiniteQuery, useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import { api, ApiError } from "./client";
import type { components } from "./schema";
import { unwrap } from "./unwrap";

export type ApiKey = components["schemas"]["ApiKey"];
export type AccessBinding = components["schemas"]["AccessBinding"];
export type AuditEntry = components["schemas"]["AuditEntry"];
export type AdminUser = components["schemas"]["AdminUser"];
export type Member = components["schemas"]["Member"];
export type CreatedKey = components["schemas"]["CreatedKey"];
export type PatchWorkspaceRequest = components["schemas"]["PatchWorkspaceRequest"];

const retry = (count: number, err: unknown) =>
  !(err instanceof ApiError && err.status !== undefined && err.status >= 400 && err.status < 500) &&
  count < 1;

export const accountKeys = {
  myKeys: ["me", "api-keys"] as const,
  access: (ws: string) => ["workspaces", ws, "access"] as const,
  members: (org: string) => ["orgs", org, "members"] as const,
  wsKeys: (ws: string) => ["workspaces", ws, "api-keys"] as const,
  audit: (ws: string, action: string) => ["workspaces", ws, "audit", action] as const,
  adminUsers: (q: string) => ["admin", "users", q] as const,
};

/* ---- account ---- */

export function useUpdateMe() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (body: components["schemas"]["PatchMeRequest"]) =>
      unwrap(api.PATCH("/v1/me", { body })),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["me"] }),
  });
}

export const revokeOtherSessions = () => unwrap(api.POST("/v1/auth/sessions/revoke-others"));

/** 412 `reauth_required` unless the session re-authenticated through OIDC within 10 minutes. */
export const deleteAccount = () => unwrap(api.DELETE("/v1/me"));

export function useMyApiKeys() {
  return useQuery({
    queryKey: accountKeys.myKeys,
    queryFn: () => unwrap(api.GET("/v1/me/api-keys", { params: { query: { limit: 200 } } })),
    retry,
  });
}

export function useRevokeApiKey(invalidate: readonly unknown[]) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (id: string) => unwrap(api.DELETE("/v1/api-keys/{id}", { params: { path: { id } } })),
    onSuccess: () => qc.invalidateQueries({ queryKey: invalidate }),
  });
}

/* ---- workspace settings ---- */

export function usePatchWorkspace(wsId: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (body: PatchWorkspaceRequest) =>
      unwrap(api.PATCH("/v1/workspaces/{ws}", { params: { path: { ws: wsId } }, body })),
    onSuccess: () => {
      void qc.invalidateQueries({ queryKey: ["workspaces"] });
      void qc.invalidateQueries({ queryKey: ["orgs"] });
      void qc.invalidateQueries({ queryKey: ["resolve"] });
      void qc.invalidateQueries({ queryKey: ["me"] });
    },
  });
}

export const deleteWorkspace = (wsId: string, confirm: string) =>
  unwrap(api.DELETE("/v1/workspaces/{ws}", { params: { path: { ws: wsId } }, body: { confirm } }));

export function useAccess(wsId: string, enabled = true) {
  return useQuery({
    queryKey: accountKeys.access(wsId),
    queryFn: () =>
      unwrap(
        api.GET("/v1/workspaces/{ws}/access", { params: { path: { ws: wsId } } }),
      ),
    enabled,
    retry,
  });
}

/** Org member list; 403 for plain members (the picker is then hidden). */
export function useOrgMembers(orgId: string | undefined) {
  return useQuery({
    queryKey: accountKeys.members(orgId ?? ""),
    queryFn: () =>
      unwrap(
        api.GET("/v1/orgs/{org}/members", {
          params: { path: { org: orgId! }, query: { limit: 200, status: "active" } },
        }),
      ),
    enabled: !!orgId,
    retry: false,
  });
}

export function usePutAccess(wsId: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (v: { user: string; role: "owner" | "approver" | "member" }) =>
      unwrap(
        api.PUT("/v1/workspaces/{ws}/access/{user}", {
          params: { path: { ws: wsId, user: v.user } },
          body: { role: v.role },
        }),
      ),
    onSuccess: () => qc.invalidateQueries({ queryKey: accountKeys.access(wsId) }),
  });
}

export function useDeleteAccess(wsId: string) {
  const qc = useQueryClient();
  return useMutation({
    // RFE-021: `reassign_to` reassigns the person's schedules and webhooks of this workspace.
    mutationFn: (v: string | { user: string; reassign_to?: string }) => {
      const { user, reassign_to } = typeof v === "string" ? { user: v, reassign_to: undefined } : v;
      return unwrap(
        api.DELETE("/v1/workspaces/{ws}/access/{user}", {
          params: { path: { ws: wsId, user }, query: reassign_to ? { reassign_to } : {} },
        }),
      );
    },
    onSuccess: () => qc.invalidateQueries({ queryKey: accountKeys.access(wsId) }),
  });
}

export function useWorkspaceKeys(wsId: string) {
  return useQuery({
    queryKey: accountKeys.wsKeys(wsId),
    queryFn: () =>
      unwrap(
        api.GET("/v1/workspaces/{ws}/api-keys", {
          params: { path: { ws: wsId }, query: { limit: 200 } },
        }),
      ),
    retry,
  });
}

/**
 * Create a workspace key. The one-time `key` is returned to the caller only
 * (use `mutateAsync`); it is never put in the query cache.
 */
export function useCreateWorkspaceKey(wsId: string) {
  const qc = useQueryClient();
  return useMutation({
    gcTime: 0,
    mutationFn: (body: components["schemas"]["CreateKeyRequest"]) =>
      unwrap(api.POST("/v1/workspaces/{ws}/api-keys", { params: { path: { ws: wsId } }, body })),
    onSuccess: () => qc.invalidateQueries({ queryKey: accountKeys.wsKeys(wsId) }),
  });
}

export function useAudit(wsId: string, action: string) {
  return useInfiniteQuery({
    queryKey: accountKeys.audit(wsId, action),
    initialPageParam: undefined as string | undefined,
    queryFn: ({ pageParam }) =>
      unwrap(
        api.GET("/v1/workspaces/{ws}/audit", {
          params: {
            path: { ws: wsId },
            query: { limit: 50, cursor: pageParam, action: action || undefined },
          },
        }),
      ),
    getNextPageParam: (last) => last.next_cursor ?? undefined,
    retry,
  });
}

/* ---- admin ---- */

export function useAdminUsers(q: string, enabled = true) {
  return useInfiniteQuery({
    enabled,
    queryKey: accountKeys.adminUsers(q),
    initialPageParam: undefined as string | undefined,
    queryFn: ({ pageParam }) =>
      unwrap(
        api.GET("/v1/admin/users", {
          params: { query: { q: q || undefined, limit: 50, cursor: pageParam } },
        }),
      ),
    getNextPageParam: (last) => last.next_cursor ?? undefined,
    retry,
  });
}

export function usePatchAdminUser() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (v: { id: string; body: components["schemas"]["PatchAdminUserRequest"] }) =>
      unwrap(api.PATCH("/v1/admin/users/{id}", { params: { path: { id: v.id } }, body: v.body })),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["admin", "users"] }),
  });
}
