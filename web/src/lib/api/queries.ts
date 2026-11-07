"use client";

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import { api, ApiError } from "./client";
import type { components } from "./schema";
import { unwrap } from "./unwrap";

export type Org = components["schemas"]["Org"];
export type Workspace = components["schemas"]["Workspace"];
export type InstanceInfo = components["schemas"]["InstanceInfo"];
export type Me = components["schemas"]["Me"];
export type Agent = components["schemas"]["Agent"];
export type CreateOrgRequest = components["schemas"]["CreateOrgRequest"];
export type CreateWorkspaceRequest = components["schemas"]["CreateWorkspaceRequest"];

export const queryKeys = {
  orgs: ["orgs"] as const,
  org: (orgId: string) => ["orgs", orgId] as const,
  workspaces: (orgId: string) => ["orgs", orgId, "workspaces"] as const,
  workspace: (wsId: string) => ["workspaces", wsId] as const,
  resolve: (org: string, ws?: string) => ["resolve", org, ws ?? null] as const,
  agents: (wsId: string) => ["workspaces", wsId, "agents"] as const,
};

/** Do not retry on definitive client errors. */
const retry = (count: number, err: unknown) =>
  !(err instanceof ApiError && err.status !== undefined && err.status >= 400 && err.status < 500) &&
  count < 1;

export function useOrgs() {
  return useQuery({
    queryKey: queryKeys.orgs,
    queryFn: () => unwrap(api.GET("/v1/orgs")),
    retry,
  });
}

export function useOrg(orgId: string | undefined) {
  return useQuery({
    queryKey: queryKeys.org(orgId ?? ""),
    queryFn: () => unwrap(api.GET("/v1/orgs/{org}", { params: { path: { org: orgId! } } })),
    enabled: !!orgId,
    retry,
  });
}

export function useWorkspaces(orgId: string | undefined) {
  return useQuery({
    queryKey: queryKeys.workspaces(orgId ?? ""),
    queryFn: () =>
      unwrap(api.GET("/v1/orgs/{org}/workspaces", { params: { path: { org: orgId! } } })),
    enabled: !!orgId,
    retry,
  });
}

export function useWorkspace(wsId: string | undefined) {
  return useQuery({
    queryKey: queryKeys.workspace(wsId ?? ""),
    queryFn: () => unwrap(api.GET("/v1/workspaces/{ws}", { params: { path: { ws: wsId! } } })),
    enabled: !!wsId,
    retry,
  });
}

/** Map URL slugs to ids (`GET /v1/resolve`). */
export function useResolve(orgSlug: string | undefined, wsSlug?: string) {
  return useQuery({
    queryKey: queryKeys.resolve(orgSlug ?? "", wsSlug),
    queryFn: () =>
      unwrap(api.GET("/v1/resolve", { params: { query: { org: orgSlug!, workspace: wsSlug } } })),
    enabled: !!orgSlug,
    retry,
  });
}

export function useAgents(wsId: string | undefined) {
  return useQuery({
    queryKey: queryKeys.agents(wsId ?? ""),
    queryFn: () =>
      unwrap(api.GET("/v1/workspaces/{ws}/agents", { params: { path: { ws: wsId! } } })),
    enabled: !!wsId,
    retry,
  });
}

export function useCreateOrg() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (body: CreateOrgRequest) => unwrap(api.POST("/v1/orgs", { body })),
    onSuccess: () => qc.invalidateQueries({ queryKey: queryKeys.orgs }),
  });
}

export function useCreateWorkspace(orgId: string | undefined) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (body: CreateWorkspaceRequest) =>
      unwrap(api.POST("/v1/orgs/{org}/workspaces", { params: { path: { org: orgId! } }, body })),
    onSuccess: () => qc.invalidateQueries({ queryKey: queryKeys.workspaces(orgId ?? "") }),
  });
}

/** `POST /v1/auth/logout` (not in the generated schema until BE-011 lands). */
export async function logout(): Promise<void> {
  const res = await fetch("/v1/auth/logout", { method: "POST", credentials: "same-origin" });
  if (!res.ok && res.status !== 401) {
    throw ApiError.fromBody(await res.json().catch(() => null), res.status);
  }
}

export interface MeInfo {
  user: { display_name: string; email: string; is_instance_admin?: boolean };
}

/**
 * `GET /v1/me` (BE-016). Not in the generated schema yet: any non-2xx
 * (404/405 before it exists) yields `null` so the UI simply omits the user.
 */
export function useMe() {
  return useQuery({
    queryKey: ["me"] as const,
    queryFn: async (): Promise<MeInfo | null> => {
      const res = await fetch("/v1/me", { credentials: "same-origin" });
      if (!res.ok) return null;
      const body: unknown = await res.json().catch(() => null);
      if (typeof body === "object" && body !== null && "user" in body) return body as MeInfo;
      return null;
    },
    retry: false,
  });
}

/** `GET /v1/instance` (public). */
export function useInstance(enabled = true) {
  return useQuery({
    queryKey: ["instance"] as const,
    queryFn: () => unwrap(api.GET("/v1/instance")),
    enabled,
    retry,
  });
}

/** `GET /v1/me`; unlike `useMe`, errors (401 = signed out) surface to the caller. */
export function useSession() {
  return useQuery({
    queryKey: ["me", "session"] as const,
    queryFn: () => unwrap(api.GET("/v1/me")),
    retry: false,
    staleTime: 0,
  });
}
