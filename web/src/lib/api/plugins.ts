"use client";

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import { api, ApiError } from "./client";
import { retryUnlessClientError } from "./errors";
import { useInvalidatingMutation } from "./mutations";
import type { components } from "./schema";
import { unwrap } from "./unwrap";

export type PluginPackage = components["schemas"]["PluginPackage"];
export type PluginViolation = components["schemas"]["PluginViolation"];
export type HostClaimConflict = components["schemas"]["HostClaimConflict"];
export type TrustedKey = components["schemas"]["TrustedKey"];

export const pluginKeys = {
  all: ["admin", "plugins"] as const,
  list: (status: string) => ["admin", "plugins", "list", status] as const,
  one: (id: string) => ["admin", "plugins", "one", id] as const,
  claims: ["admin", "host-claims"] as const,
  keys: ["admin", "trusted-keys"] as const,
};

export function usePlugins(status: string) {
  return useQuery({
    queryKey: pluginKeys.list(status),
    queryFn: () =>
      unwrap(
        api.GET("/v1/admin/plugins", {
          params: { query: { ...(status ? { status } : {}), limit: 200 } },
        }),
      ),
    retry: retryUnlessClientError,
  });
}

export function usePlugin(id: string) {
  return useQuery({
    queryKey: pluginKeys.one(id),
    queryFn: () => unwrap(api.GET("/v1/admin/plugins/{id}", { params: { path: { id } } })),
    retry: retryUnlessClientError,
  });
}

/** A package version's id is a UUID; a package id (`atlassian`) is not (FIX-201, D-438). */
export const isVersionId = (id: string) => /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(id);

/**
 * (FIX-201, C-1120) The versions of one package by its package id, newest first. Disabled for a UUID: `usePlugin`
 * is never called with a package id, and this is never called with a version id.
 */
export function usePluginVersions(packageId: string) {
  return useQuery({
    queryKey: [...pluginKeys.all, "versions", packageId] as const,
    queryFn: () =>
      unwrap(api.GET("/v1/admin/plugins", { params: { query: { package_id: packageId, limit: 200 } } })),
    enabled: !isVersionId(packageId),
    retry: retryUnlessClientError,
  });
}

export type PluginAgent = components["schemas"]["PluginAgent"];

/** A file an administrator can open: a file of an app skill (`SKILL.md` when `path` is null) or the operations file. */
export type AgentFileRef = { kind: "skill"; name: string; path: string | null } | { kind: "operations" };

/**
 * (P5-APPSKILL-2, C-1136) One agent file of a package as text. Fetched only while `ref` is set (a file is
 * selected): the content is written by the publisher, so it is never parsed or rendered as anything but text.
 */
export function useAgentFile(id: string, appId: string, ref: AgentFileRef | null) {
  const key = ref === null ? null : ref.kind === "operations" ? "operations" : `${ref.name}/${ref.path ?? "SKILL.md"}`;
  return useQuery({
    queryKey: [...pluginKeys.one(id), "agent-file", appId, key] as const,
    queryFn: async (): Promise<string> => {
      const r = ref!;
      const { data, response } =
        r.kind === "operations"
          ? await api.GET("/v1/admin/plugins/{id}/apps/{app_id}/operations", {
              params: { path: { id, app_id: appId } },
              parseAs: "text",
            })
          : await api.GET("/v1/admin/plugins/{id}/apps/{app_id}/skills/{name}/file", {
              params: { path: { id, app_id: appId, name: r.name }, query: r.path === null ? {} : { path: r.path } },
              parseAs: "text",
            });
      if (response.status === 404) throw new ApiError("not_found", "This file is no longer available.", null, 404);
      if (!response.ok) throw new ApiError("unknown", "Could not load the file.", null, response.status);
      return typeof data === "string" ? data : "";
    },
    enabled: ref !== null,
    retry: false,
    staleTime: Infinity,
  });
}

export function useUploadPlugin() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (file: File) => {
      return unwrap(
        api.POST("/v1/admin/plugins", {
          // eslint-disable-next-line @typescript-eslint/no-explicit-any
          body: { package: file } as any,
          bodySerializer: () => {
            const fd = new FormData();
            fd.append("package", file);
            return fd;
          },
        }),
      );
    },
    onSettled: () => qc.invalidateQueries({ queryKey: pluginKeys.all }),
  });
}

export type PluginAction = "approve" | "reject" | "disable" | "enable";

export function usePluginAction(id: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({ action, reason }: { action: PluginAction; reason?: string }) =>
      unwrap(
        api.POST(`/v1/admin/plugins/{id}/${action}` as "/v1/admin/plugins/{id}/approve", {
          params: { path: { id } },
          body: reason ? { reason } : {},
        }),
      ),
    onSuccess: () => qc.invalidateQueries({ queryKey: pluginKeys.all }),
  });
}

export function useUninstallPlugin(id: string) {
  return useInvalidatingMutation((force: boolean) =>
    unwrap(
      api.DELETE("/v1/admin/plugins/{id}", {
        params: { path: { id }, query: force ? { force: true } : {} },
      }),
    ), pluginKeys.all);
}

export function useHostClaims() {
  return useQuery({
    queryKey: pluginKeys.claims,
    queryFn: () => unwrap(api.GET("/v1/admin/host-claims")),
    retry: retryUnlessClientError,
  });
}

export function useDecideHostClaim() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (v: {
      group_key: string;
      winner_package_id: string;
      winner_app_id: string;
      fingerprint: string;
    }) =>
      unwrap(
        api.PUT("/v1/admin/host-claims/{group_key}", {
          params: { path: { group_key: v.group_key } },
          body: {
            winner_package_id: v.winner_package_id,
            winner_app_id: v.winner_app_id,
            fingerprint: v.fingerprint,
          },
        }),
      ),
    onSettled: () => qc.invalidateQueries({ queryKey: pluginKeys.claims }),
  });
}

export function useTrustedKeys() {
  return useQuery({
    queryKey: pluginKeys.keys,
    queryFn: () => unwrap(api.GET("/v1/admin/trusted-keys", { params: { query: { limit: 200 } } })),
    retry: retryUnlessClientError,
  });
}

export function useAddTrustedKey() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (body: components["schemas"]["CreateTrustedKeyRequest"]) =>
      unwrap(api.POST("/v1/admin/trusted-keys", { body })),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["admin"] }),
  });
}

export function useRevokeTrustedKey() {
  return useInvalidatingMutation((id: string) =>
    unwrap(api.DELETE("/v1/admin/trusted-keys/{id}", { params: { path: { id } } })), ["admin"]);
}
