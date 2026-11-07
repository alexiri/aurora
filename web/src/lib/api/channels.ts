"use client";

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import { api, ApiError } from "./client";
import type { components } from "./schema";
import { unwrap } from "./unwrap";

type S = components["schemas"];
export type ChannelIntegration = S["ChannelIntegration"];
export type Presence = S["Presence"];
export type PresenceOption = S["PresenceOption"];
export type PresenceMode = S["PresenceMode"];
export type SpaceBinding = S["SpaceBinding"];
export type UserLink = S["UserLink"];
export type DmDefault = S["DmDefault"];
export type ReachGrant = S["ReachGrant"];
export type Listen = S["Listen"];

const retry = (count: number, err: unknown) =>
  !(err instanceof ApiError && err.status !== undefined && err.status >= 400 && err.status < 500) &&
  count < 1;

export const channelKeys = {
  integrations: (org: string) => ["orgs", org, "channel-integrations"] as const,
  userLinks: (integration: string) => ["channel-integrations", integration, "user-links"] as const,
  presences: (agent: string) => ["agents", agent, "presences"] as const,
  presenceOptions: (agent: string) => ["agents", agent, "presence-options"] as const,
  bindings: (agent: string) => ["agents", agent, "space-bindings"] as const,
  reach: (agent: string) => ["agents", agent, "reach"] as const,
  myLinks: ["me", "channel-links"] as const,
  dmDefaults: ["me", "dm-defaults"] as const,
};

export function useIntegrations(orgId: string | undefined) {
  return useQuery({
    queryKey: channelKeys.integrations(orgId ?? ""),
    queryFn: () =>
      unwrap(
        api.GET("/v1/orgs/{org}/channel-integrations", {
          params: { path: { org: orgId! }, query: { limit: 200 } },
        }),
      ),
    enabled: !!orgId,
    retry,
  });
}

export function useCreateIntegration(orgId: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (body: S["CreateIntegrationRequest"]) =>
      unwrap(api.POST("/v1/orgs/{org}/channel-integrations", { params: { path: { org: orgId } }, body })),
    onSuccess: () => qc.invalidateQueries({ queryKey: channelKeys.integrations(orgId) }),
  });
}

export function usePatchIntegration(orgId: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({ id, body }: { id: string; body: S["PatchIntegrationRequest"] }) =>
      unwrap(api.PATCH("/v1/channel-integrations/{id}", { params: { path: { id } }, body })),
    onSettled: () => qc.invalidateQueries({ queryKey: channelKeys.integrations(orgId) }),
  });
}

export function useDeleteIntegration(orgId: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (id: string) =>
      unwrap(api.DELETE("/v1/channel-integrations/{id}", { params: { path: { id } } })),
    onSuccess: () => qc.invalidateQueries({ queryKey: channelKeys.integrations(orgId) }),
  });
}

export function useIntegrationUserLinks(integration: string, enabled: boolean) {
  return useQuery({
    queryKey: channelKeys.userLinks(integration),
    queryFn: () =>
      unwrap(
        api.GET("/v1/channel-integrations/{id}/user-links", {
          params: { path: { id: integration }, query: { limit: 200 } },
        }),
      ),
    enabled,
    retry,
  });
}

export function useDeleteUserLink(integration: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (id: string) => unwrap(api.DELETE("/v1/user-links/{id}", { params: { path: { id } } })),
    onSuccess: () => qc.invalidateQueries({ queryKey: channelKeys.userLinks(integration) }),
  });
}

export function usePresences(agent: string) {
  return useQuery({
    queryKey: channelKeys.presences(agent),
    queryFn: () => unwrap(api.GET("/v1/agents/{agent}/presences", { params: { path: { agent } } })),
    retry,
  });
}

export function usePresenceOptions(agent: string) {
  return useQuery({
    queryKey: channelKeys.presenceOptions(agent),
    queryFn: () => unwrap(api.GET("/v1/agents/{agent}/presence-options", { params: { path: { agent } } })),
    retry,
  });
}

export function useAttachPresence(agent: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (body: S["AttachPresenceRequest"]) =>
      unwrap(api.POST("/v1/agents/{agent}/presences", { params: { path: { agent } }, body })),
    onSuccess: () => Promise.all([qc.invalidateQueries({ queryKey: channelKeys.presences(agent) }), qc.invalidateQueries({ queryKey: channelKeys.presenceOptions(agent) })]),
  });
}

export function useReattachPresence(agent: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({ id, fields }: { id: string; fields?: Record<string, string> }) =>
      unwrap(
        api.POST("/v1/presences/{id}/reattach", {
          params: { path: { id } },
          body: fields ? { fields } : {},
        }),
      ),
    onSettled: () => Promise.all([qc.invalidateQueries({ queryKey: channelKeys.presences(agent) }), qc.invalidateQueries({ queryKey: channelKeys.presenceOptions(agent) })]),
  });
}

export function useDetachPresence(agent: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (id: string) => unwrap(api.DELETE("/v1/presences/{id}", { params: { path: { id } } })),
    onSuccess: () => Promise.all([qc.invalidateQueries({ queryKey: channelKeys.presences(agent) }), qc.invalidateQueries({ queryKey: channelKeys.presenceOptions(agent) })]),
  });
}

export function useSpaceBindings(agent: string, enabled = true) {
  return useQuery({
    queryKey: channelKeys.bindings(agent),
    queryFn: () => unwrap(api.GET("/v1/agents/{agent}/space-bindings", { params: { path: { agent } } })),
    enabled,
    retry,
  });
}

export function usePatchSpaceBinding(agent: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({ id, body }: { id: string; body: S["PatchBindingRequest"] }) =>
      unwrap(api.PATCH("/v1/space-bindings/{id}", { params: { path: { id } }, body })),
    onSuccess: () => qc.invalidateQueries({ queryKey: channelKeys.bindings(agent) }),
  });
}

export function useDeleteSpaceBinding(agent: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (id: string) => unwrap(api.DELETE("/v1/space-bindings/{id}", { params: { path: { id } } })),
    onSuccess: () => qc.invalidateQueries({ queryKey: channelKeys.bindings(agent) }),
  });
}

export function useReach(agent: string) {
  return useQuery({
    queryKey: channelKeys.reach(agent),
    queryFn: () =>
      unwrap(api.GET("/v1/agents/{agent}/reach", { params: { path: { agent }, query: { limit: 100 } } })),
    retry,
  });
}

export function useDecideReach(agent: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({ id, body }: { id: string; body: S["ReachDecisionRequest"] }) =>
      unwrap(api.POST("/v1/reach-grants/{id}/decision", { params: { path: { id } }, body })),
    // A 409 stale_revision needs the fresh row, so refetch whatever the outcome.
    onSettled: () => qc.invalidateQueries({ queryKey: channelKeys.reach(agent) }),
  });
}

export function useMyChannelLinks() {
  return useQuery({
    queryKey: channelKeys.myLinks,
    queryFn: () => unwrap(api.GET("/v1/me/channel-links")),
    retry,
  });
}

export function useUnlinkMine() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (id: string) =>
      unwrap(api.DELETE("/v1/me/channel-links/{id}", { params: { path: { id } } })),
    onSuccess: () => {
      void qc.invalidateQueries({ queryKey: channelKeys.myLinks });
      void qc.invalidateQueries({ queryKey: channelKeys.dmDefaults });
    },
  });
}

export function useConfirmChannelLink() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (token: string) => unwrap(api.POST("/v1/channel-links/confirm", { body: { token } })),
    onSuccess: () => qc.invalidateQueries({ queryKey: channelKeys.myLinks }),
  });
}

export function useDmDefaults() {
  return useQuery({
    queryKey: channelKeys.dmDefaults,
    queryFn: () => unwrap(api.GET("/v1/me/dm-defaults")),
    retry,
  });
}

export function useSetDmDefault() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (body: S["SetDmDefaultRequest"]) => unwrap(api.PUT("/v1/me/dm-defaults", { body })),
    onSuccess: () => qc.invalidateQueries({ queryKey: channelKeys.dmDefaults }),
  });
}
