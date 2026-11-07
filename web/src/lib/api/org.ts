"use client";

import { useInfiniteQuery, useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import { api, ApiError } from "./client";
import type { components } from "./schema";
import { unwrap } from "./unwrap";

export type OrgMember = components["schemas"]["Member"];
export type OrgRole = components["schemas"]["OrgRole"];
export type MemberStatus = components["schemas"]["MemberStatus"];
export type Invitation = components["schemas"]["Invitation"];
export type PatchOrgRequest = components["schemas"]["PatchOrgRequest"];
export type CreateInvitationRequest = components["schemas"]["CreateInvitationRequest"];

const retry = (count: number, err: unknown) =>
  !(err instanceof ApiError && err.status !== undefined && err.status >= 400 && err.status < 500) &&
  count < 1;

export const orgKeys = {
  members: (org: string) => ["orgs", org, "members", "all"] as const,
  invitations: (org: string) => ["orgs", org, "invitations"] as const,
  keys: (org: string) => ["orgs", org, "api-keys"] as const,
  audit: (org: string, action: string) => ["orgs", org, "audit", action] as const,
};

/**
 * Roles a viewer may assign to a member (permission matrix, 04): owners can
 * set any role; admins cannot touch owners or grant owner.
 */
export function roleOptions(viewer: OrgRole, target: OrgRole): OrgRole[] {
  if (viewer === "owner") return ["owner", "admin", "member"];
  if (viewer === "admin") return target === "owner" ? [] : ["admin", "member"];
  return [];
}

/** Whether the viewer may suspend/remove the target. */
export const canManageMember = (viewer: OrgRole, target: OrgRole) =>
  viewer === "owner" || (viewer === "admin" && target !== "owner");

export interface InviteForm {
  email: string;
  role: "admin" | "member";
}

/** Client-side invite validation; backend remains authoritative. */
export function validateInvite(f: InviteForm): string | null {
  const email = f.email.trim();
  if (!email || email.length > 254 || !/^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(email)) {
    return "Enter a valid email address.";
  }
  if (f.role !== "admin" && f.role !== "member") return "Choose a role.";
  return null;
}

export function useAllMembers(orgId: string) {
  return useInfiniteQuery({
    queryKey: orgKeys.members(orgId),
    initialPageParam: undefined as string | undefined,
    queryFn: ({ pageParam }) =>
      unwrap(
        api.GET("/v1/orgs/{org}/members", {
          params: { path: { org: orgId }, query: { limit: 100, cursor: pageParam } },
        }),
      ),
    getNextPageParam: (last) => last.next_cursor ?? undefined,
    retry,
  });
}

export function usePatchMember(orgId: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (v: { user: string; body: components["schemas"]["PatchMemberRequest"] }) =>
      unwrap(
        api.PATCH("/v1/orgs/{org}/members/{user}", {
          params: { path: { org: orgId, user: v.user } },
          body: v.body,
        }),
      ),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["orgs", orgId, "members"] }),
  });
}

export function useRemoveMember(orgId: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (user: string) =>
      unwrap(
        api.DELETE("/v1/orgs/{org}/members/{user}", { params: { path: { org: orgId, user } } }),
      ),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["orgs", orgId, "members"] }),
  });
}

export function useInvitations(orgId: string) {
  return useQuery({
    queryKey: orgKeys.invitations(orgId),
    queryFn: () =>
      unwrap(
        api.GET("/v1/orgs/{org}/invitations", {
          params: { path: { org: orgId }, query: { status: "pending", limit: 200 } },
        }),
      ),
    retry,
  });
}

export function useCreateInvitation(orgId: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (body: CreateInvitationRequest) =>
      unwrap(api.POST("/v1/orgs/{org}/invitations", { params: { path: { org: orgId } }, body })),
    onSuccess: () => qc.invalidateQueries({ queryKey: orgKeys.invitations(orgId) }),
  });
}

export function useCancelInvitation(orgId: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (id: string) =>
      unwrap(api.DELETE("/v1/invitations/{id}", { params: { path: { id } } })),
    onSuccess: () => qc.invalidateQueries({ queryKey: orgKeys.invitations(orgId) }),
  });
}

export function useOrgKeys(orgId: string) {
  return useQuery({
    queryKey: orgKeys.keys(orgId),
    queryFn: () =>
      unwrap(
        api.GET("/v1/orgs/{org}/api-keys", {
          params: { path: { org: orgId }, query: { limit: 200 } },
        }),
      ),
    retry,
  });
}

/** The one-time `key` is returned to the caller only; never cached. */
export function useCreateOrgKey(orgId: string) {
  const qc = useQueryClient();
  return useMutation({
    gcTime: 0,
    mutationFn: (body: components["schemas"]["CreateKeyRequest"]) =>
      unwrap(api.POST("/v1/orgs/{org}/api-keys", { params: { path: { org: orgId } }, body })),
    onSuccess: () => qc.invalidateQueries({ queryKey: orgKeys.keys(orgId) }),
  });
}

export function useOrgAudit(orgId: string, action: string) {
  return useInfiniteQuery({
    queryKey: orgKeys.audit(orgId, action),
    initialPageParam: undefined as string | undefined,
    queryFn: ({ pageParam }) =>
      unwrap(
        api.GET("/v1/orgs/{org}/audit", {
          params: {
            path: { org: orgId },
            query: { limit: 50, cursor: pageParam, action: action || undefined },
          },
        }),
      ),
    getNextPageParam: (last) => last.next_cursor ?? undefined,
    retry,
  });
}

export function usePatchOrg(orgId: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (body: PatchOrgRequest) =>
      unwrap(api.PATCH("/v1/orgs/{org}", { params: { path: { org: orgId } }, body })),
    onSuccess: () => {
      void qc.invalidateQueries({ queryKey: ["orgs"] });
      void qc.invalidateQueries({ queryKey: ["resolve"] });
    },
  });
}

export const deleteOrg = (orgId: string, confirm: string) =>
  unwrap(api.DELETE("/v1/orgs/{org}", { params: { path: { org: orgId } }, body: { confirm } }));
