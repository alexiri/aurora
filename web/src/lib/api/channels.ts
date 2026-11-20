"use client";

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import { api } from "./client";
import { retryUnlessClientError } from "./errors";
import { useInvalidatingMutation } from "./mutations";
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
export type MySpace = S["MySpace"];

export const channelKeys = {
  integrations: (org: string) => ["orgs", org, "channel-integrations"] as const,
  userLinks: (integration: string) => ["channel-integrations", integration, "user-links"] as const,
  presences: (agent: string) => ["agents", agent, "presences"] as const,
  presenceOptions: (agent: string) => ["agents", agent, "presence-options"] as const,
  bindings: (agent: string) => ["agents", agent, "space-bindings"] as const,
  mySpaces: (integration: string) => ["channel-integrations", integration, "my-spaces"] as const,
  reach: (agent: string) => ["agents", agent, "reach"] as const,
  myLinks: ["me", "channel-links"] as const,
  myPlatforms: ["me", "chat-platforms"] as const,
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
    retry: retryUnlessClientError,
  });
}

export function useCreateIntegration(orgId: string) {
  return useInvalidatingMutation((body: S["CreateIntegrationRequest"]) =>
    unwrap(api.POST("/v1/orgs/{org}/channel-integrations", { params: { path: { org: orgId } }, body })), channelKeys.integrations(orgId));
}

/** Check credentials and list the choices a field takes from the platform (C-711); stores nothing. */
export function useProbeIntegration(orgId: string) {
  return useMutation({
    gcTime: 0,
    mutationFn: (body: S["ProbeIntegrationRequest"]) =>
      unwrap(api.POST("/v1/orgs/{org}/channel-integrations:probe", { params: { path: { org: orgId } }, body })),
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
    retry: retryUnlessClientError,
  });
}

export function useDeleteUserLink(integration: string) {
  return useInvalidatingMutation((id: string) => unwrap(api.DELETE("/v1/user-links/{id}", { params: { path: { id } } })), channelKeys.userLinks(integration));
}

export function usePresences(agent: string) {
  return useQuery({
    queryKey: channelKeys.presences(agent),
    queryFn: () => unwrap(api.GET("/v1/agents/{agent}/presences", { params: { path: { agent } } })),
    retry: retryUnlessClientError,
  });
}

export function usePresenceOptions(agent: string) {
  return useQuery({
    queryKey: channelKeys.presenceOptions(agent),
    queryFn: () => unwrap(api.GET("/v1/agents/{agent}/presence-options", { params: { path: { agent } } })),
    retry: retryUnlessClientError,
  });
}

export function useAttachPresence(agent: string) {
  return useInvalidatingMutation((body: S["AttachPresenceRequest"]) =>
    unwrap(api.POST("/v1/agents/{agent}/presences", { params: { path: { agent } }, body })), channelKeys.presences(agent), channelKeys.presenceOptions(agent));
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
    retry: retryUnlessClientError,
  });
}

/** The spaces the caller is in where the integration's shared bot is present (C-714). */
export function useMySpaces(integration: string, enabled: boolean) {
  return useQuery({
    queryKey: channelKeys.mySpaces(integration),
    queryFn: () =>
      unwrap(
        api.GET("/v1/integrations/{id}/spaces", {
          params: { path: { id: integration }, query: { mine: true } },
        }),
      ),
    enabled,
    retry: retryUnlessClientError,
    // A person invites the bot in Mattermost and comes back: always ask the platform again.
    staleTime: 0,
    gcTime: 0,
  });
}

/** Bind the agent to a space from the dashboard (C-713). */
export function useCreateSpaceBinding(agent: string) {
  return useInvalidatingMutation((body: S["CreateBindingRequest"]) =>
    unwrap(api.POST("/v1/agents/{agent}/space-bindings", { params: { path: { agent } }, body })), channelKeys.bindings(agent));
}

export function usePatchSpaceBinding(agent: string) {
  return useInvalidatingMutation(({ id, body }: { id: string; body: S["PatchBindingRequest"] }) =>
    unwrap(api.PATCH("/v1/space-bindings/{id}", { params: { path: { id } }, body })), channelKeys.bindings(agent));
}

export function useDeleteSpaceBinding(agent: string) {
  return useInvalidatingMutation((id: string) => unwrap(api.DELETE("/v1/space-bindings/{id}", { params: { path: { id } } })), channelKeys.bindings(agent));
}

export function useReach(agent: string) {
  return useQuery({
    queryKey: channelKeys.reach(agent),
    queryFn: () =>
      unwrap(api.GET("/v1/agents/{agent}/reach", { params: { path: { agent }, query: { limit: 100 } } })),
    retry: retryUnlessClientError,
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

/** (C-1089) The chat platforms the person can link, with the bot to message: any signed-in person may read it. */
export function useMyChatPlatforms() {
  return useQuery({
    queryKey: channelKeys.myPlatforms,
    queryFn: () => unwrap(api.GET("/v1/me/chat-platforms")),
    retry: retryUnlessClientError,
  });
}

export function useMyChannelLinks() {
  return useQuery({
    queryKey: channelKeys.myLinks,
    queryFn: () => unwrap(api.GET("/v1/me/channel-links")),
    retry: retryUnlessClientError,
  });
}

export function useUnlinkMine() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (id: string) =>
      unwrap(api.DELETE("/v1/me/channel-links/{id}", { params: { path: { id } } })),
    onSuccess: () => {
      void qc.invalidateQueries({ queryKey: channelKeys.myLinks });
      void qc.invalidateQueries({ queryKey: channelKeys.myPlatforms });
      void qc.invalidateQueries({ queryKey: channelKeys.dmDefaults });
    },
  });
}

/** The chat account a sign-in link token would bind (shown before the person confirms). */
export function useChannelLinkPreview(token: string | null, enabled: boolean) {
  return useQuery({
    queryKey: ["channel-link-preview", token] as const,
    queryFn: () => unwrap(api.GET("/v1/channel-links/preview", { params: { query: { token: token! } } })),
    enabled: enabled && !!token,
    retry: false,
    staleTime: Infinity,
  });
}

export function useConfirmChannelLink() {
  const qc = useQueryClient();
  return useMutation({
    // The request repeats the platform user the page showed; the server refuses a mismatch.
    mutationFn: (v: { token: string; platform_user_ref: string }) =>
      unwrap(api.POST("/v1/channel-links/confirm", { body: v })),
    onSuccess: () => qc.invalidateQueries({ queryKey: channelKeys.myLinks }),
  });
}

export function useDmDefaults() {
  return useQuery({
    queryKey: channelKeys.dmDefaults,
    queryFn: () => unwrap(api.GET("/v1/me/dm-defaults")),
    retry: retryUnlessClientError,
  });
}

export function useSetDmDefault() {
  return useInvalidatingMutation((body: S["SetDmDefaultRequest"]) => unwrap(api.PUT("/v1/me/dm-defaults", { body })), channelKeys.dmDefaults);
}
