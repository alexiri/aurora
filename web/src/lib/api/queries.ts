"use client";

import { useQuery } from "@tanstack/react-query";

import { api, ApiError } from "./client";
import { retryUnlessClientError } from "./errors";
import { useInvalidatingMutation } from "./mutations";
import { fetchAllPages, PAGE_LIMIT } from "./paging";
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

export function useOrgs() {
  return useQuery({
    queryKey: queryKeys.orgs,
    queryFn: () => unwrap(api.GET("/v1/orgs")),
    retry: retryUnlessClientError,
  });
}

export function useOrg(orgId: string | undefined) {
  return useQuery({
    queryKey: queryKeys.org(orgId ?? ""),
    queryFn: () => unwrap(api.GET("/v1/orgs/{org}", { params: { path: { org: orgId! } } })),
    enabled: !!orgId,
    retry: retryUnlessClientError,
  });
}

export function useWorkspaces(orgId: string | undefined) {
  return useQuery({
    queryKey: queryKeys.workspaces(orgId ?? ""),
    // Every page: a workspace past the 50th must still be bindable to a group (FIX-117).
    queryFn: () =>
      fetchAllPages((cursor) =>
        unwrap(
          api.GET("/v1/orgs/{org}/workspaces", {
            params: { path: { org: orgId! }, query: { limit: PAGE_LIMIT, cursor } },
          }),
        ),
      ),
    enabled: !!orgId,
    retry: retryUnlessClientError,
  });
}

export function useWorkspace(wsId: string | undefined) {
  return useQuery({
    queryKey: queryKeys.workspace(wsId ?? ""),
    queryFn: () => unwrap(api.GET("/v1/workspaces/{ws}", { params: { path: { ws: wsId! } } })),
    enabled: !!wsId,
    retry: retryUnlessClientError,
  });
}

/** Map URL slugs to ids (`GET /v1/resolve`). */
export function useResolve(orgSlug: string | undefined, wsSlug?: string) {
  return useQuery({
    queryKey: queryKeys.resolve(orgSlug ?? "", wsSlug),
    queryFn: () =>
      unwrap(api.GET("/v1/resolve", { params: { query: { org: orgSlug!, workspace: wsSlug } } })),
    enabled: !!orgSlug,
    retry: retryUnlessClientError,
    // (P5-REFINE-20, org-3) A 404 is the answer ("there is no such organization"), not a failure to try again: every new
    // observer that mounts (the shell shows the not-found state, the page frame mounts again) must not refetch it, or the
    // error is cleared, the page renders, mounts a fresh observer, and the loop never ends.
    retryOnMount: false,
  });
}

export function useAgents(wsId: string | undefined) {
  return useQuery({
    queryKey: queryKeys.agents(wsId ?? ""),
    queryFn: () =>
      unwrap(api.GET("/v1/workspaces/{ws}/agents", { params: { path: { ws: wsId! } } })),
    enabled: !!wsId,
    retry: retryUnlessClientError,
  });
}

export function useCreateOrg() {
  return useInvalidatingMutation((body: CreateOrgRequest) => unwrap(api.POST("/v1/orgs", { body })), queryKeys.orgs);
}

export function useCreateWorkspace(orgId: string | undefined) {
  return useInvalidatingMutation((body: CreateWorkspaceRequest) =>
    unwrap(api.POST("/v1/orgs/{org}/workspaces", { params: { path: { org: orgId! } }, body })), queryKeys.workspaces(orgId ?? ""));
}

/** `POST /v1/auth/logout` (not in the generated schema until BE-011 lands). */
export async function logout(): Promise<void> {
  const res = await fetch("/v1/auth/logout", { method: "POST", credentials: "same-origin" });
  if (!res.ok && res.status !== 401) {
    throw ApiError.fromBody(await res.json().catch(() => null), res.status);
  }
}

export interface MeInfo {
  user: { id?: string; display_name: string; email: string; is_instance_admin?: boolean };
  /** The body's memberships, with each workspace the person can use and whether it is their personal one (C-531). */
  memberships?: {
    org: { id?: string; slug: string; name?: string };
    role?: string;
    workspaces: { id?: string; slug: string; name?: string; my_role?: string; personal?: boolean }[];
  }[];
  /** (P5-PERSON, C-1054) The person's own IANA time zone; `null` when unset (absent on an older API). */
  timezone?: string | null;
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
    retry: retryUnlessClientError,
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
