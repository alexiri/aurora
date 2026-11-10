"use client";

import { useInfiniteQuery, useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import { api, ApiError } from "./client";
import type { components } from "./schema";
import { unwrap } from "./unwrap";

export type Group = components["schemas"]["Group"];
export type GroupMember = components["schemas"]["GroupMember"];
export type GroupBinding = components["schemas"]["GroupBinding"];
export type GroupImpact = components["schemas"]["GroupImpact"];
export type GroupSource = components["schemas"]["GroupSource"];
export type CreateGroupRequest = components["schemas"]["CreateGroupRequest"];
export type PatchGroupRequest = components["schemas"]["PatchGroupRequest"];
export type GroupAccessBinding = components["schemas"]["GroupAccessBinding"];
export type DerivedMember = components["schemas"]["Page_DerivedMemberView"]["items"][number];
export type AccessSource = components["schemas"]["AccessSourceView"];
export type WorkspaceRole = "owner" | "approver" | "member";

const retry = (count: number, err: unknown) =>
  !(err instanceof ApiError && err.status !== undefined && err.status >= 400 && err.status < 500) &&
  count < 1;

export const groupKeys = {
  list: (org: string) => ["orgs", org, "groups"] as const,
  one: (id: string) => ["groups", id] as const,
  members: (id: string) => ["groups", id, "members"] as const,
  workspaces: (id: string) => ["groups", id, "workspaces"] as const,
  impact: (id: string, member: string | null) => ["groups", id, "impact", member] as const,
};

/** The source label of a group (`docs/14`): Manual, SCIM or Sign-in (D-156). */
export function sourceLabel(s: GroupSource): string {
  return s === "scim" ? "SCIM" : s === "idp" ? "Sign-in" : "Manual";
}

/** Members of the group are edited by the IdP, not by hand. */
export const isManaged = (g: Pick<Group, "source">) => g.source !== "manual";

/** Groups of an org (names and counts; org owners/admins, or workspace owners for the picker). */
export function useGroups(orgId: string | undefined, q = "") {
  return useQuery({
    queryKey: [...groupKeys.list(orgId ?? ""), q] as const,
    queryFn: () =>
      unwrap(
        api.GET("/v1/orgs/{org}/groups", {
          params: { path: { org: orgId! }, query: { limit: 200, q: q || undefined } },
        }),
      ),
    enabled: !!orgId,
    retry,
  });
}

export function useGroup(id: string) {
  return useQuery({
    queryKey: groupKeys.one(id),
    queryFn: () => unwrap(api.GET("/v1/groups/{id}", { params: { path: { id } } })),
    retry,
  });
}

export function useGroupMembers(id: string) {
  return useInfiniteQuery({
    queryKey: groupKeys.members(id),
    initialPageParam: undefined as string | undefined,
    queryFn: ({ pageParam }) =>
      unwrap(
        api.GET("/v1/groups/{id}/members", {
          params: { path: { id }, query: { limit: 100, cursor: pageParam } },
        }),
      ),
    getNextPageParam: (last) => last.next_cursor ?? undefined,
    retry,
  });
}

export function useGroupWorkspaces(id: string) {
  return useQuery({
    queryKey: groupKeys.workspaces(id),
    queryFn: () =>
      unwrap(
        api.GET("/v1/groups/{id}/workspaces", { params: { path: { id }, query: { limit: 200 } } }),
      ),
    retry,
  });
}

/** What deleting the group (or removing one member) would take away (`05 §Groups`). */
export function useGroupImpact(id: string, removeMember: string | null, enabled: boolean) {
  return useQuery({
    queryKey: groupKeys.impact(id, removeMember),
    queryFn: () =>
      unwrap(
        api.GET("/v1/groups/{id}/impact", {
          params: { path: { id }, query: removeMember ? { remove_member: removeMember } : {} },
        }),
      ),
    enabled,
    retry: false,
    gcTime: 0,
  });
}

export function useCreateGroup(orgId: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (body: CreateGroupRequest) =>
      unwrap(api.POST("/v1/orgs/{org}/groups", { params: { path: { org: orgId } }, body })),
    onSuccess: () => qc.invalidateQueries({ queryKey: groupKeys.list(orgId) }),
  });
}

export function usePatchGroup(id: string, orgId: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (body: PatchGroupRequest) =>
      unwrap(api.PATCH("/v1/groups/{id}", { params: { path: { id } }, body })),
    onSuccess: (g) => {
      qc.setQueryData(groupKeys.one(id), g);
      void qc.invalidateQueries({ queryKey: groupKeys.list(orgId) });
    },
  });
}

export function useDeleteGroup(id: string, orgId: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: () => unwrap(api.DELETE("/v1/groups/{id}", { params: { path: { id } } })),
    onSuccess: () => {
      qc.removeQueries({ queryKey: groupKeys.one(id) });
      void qc.invalidateQueries({ queryKey: groupKeys.list(orgId) });
    },
  });
}

export function useChangeMembers(id: string, orgId: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (v: { add?: string[]; remove?: string[] }) =>
      unwrap(
        api.POST("/v1/groups/{id}/members:batch", {
          params: { path: { id } },
          body: { add: v.add ?? [], remove: v.remove ?? [] },
        }),
      ),
    onSuccess: () => {
      void qc.invalidateQueries({ queryKey: groupKeys.members(id) });
      void qc.invalidateQueries({ queryKey: groupKeys.one(id) });
      void qc.invalidateQueries({ queryKey: groupKeys.list(orgId) });
    },
  });
}

/** Bind (or change the role of) a group on a workspace; `can_own` of the workspace. */
export function usePutGroupAccess(wsId: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (v: { group: string; role: WorkspaceRole }) =>
      unwrap(
        api.PUT("/v1/workspaces/{ws}/access/groups/{group}", {
          params: { path: { ws: wsId, group: v.group } },
          body: { role: v.role },
        }),
      ),
    onSuccess: () => {
      void qc.invalidateQueries({ queryKey: ["workspaces", wsId, "access"] });
      void qc.invalidateQueries({ queryKey: ["groups"] });
    },
  });
}

export function useDeleteGroupAccess(wsId: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (group: string) =>
      unwrap(
        api.DELETE("/v1/workspaces/{ws}/access/groups/{group}", {
          params: { path: { ws: wsId, group } },
        }),
      ),
    onSuccess: () => {
      void qc.invalidateQueries({ queryKey: ["workspaces", wsId, "access"] });
      void qc.invalidateQueries({ queryKey: ["groups"] });
    },
  });
}

/** Human text of one source of an effective role (`04 §Groups`). */
export function sourceText(s: AccessSource): string {
  switch (s.kind) {
    case "group":
      return `Group ${s.group?.name ?? ""}`.trim();
    case "org_role_via_group":
      return `Org role via ${s.group?.name ?? "a group"}`;
    case "org_role":
      return "Org admin/owner";
    case "idp_claim":
      return "From IdP role";
    default:
      return "Direct";
  }
}
