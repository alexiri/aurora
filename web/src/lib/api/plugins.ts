"use client";

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import { api, ApiError } from "./client";
import type { components } from "./schema";
import { unwrap } from "./unwrap";

export type PluginPackage = components["schemas"]["PluginPackage"];
export type PluginViolation = components["schemas"]["PluginViolation"];
export type HostClaimConflict = components["schemas"]["HostClaimConflict"];
export type TrustedKey = components["schemas"]["TrustedKey"];

const retry = (count: number, err: unknown) =>
  !(err instanceof ApiError && err.status !== undefined && err.status >= 400 && err.status < 500) &&
  count < 1;

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
    retry,
  });
}

export function usePlugin(id: string) {
  return useQuery({
    queryKey: pluginKeys.one(id),
    queryFn: () => unwrap(api.GET("/v1/admin/plugins/{id}", { params: { path: { id } } })),
    retry,
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
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (force: boolean) =>
      unwrap(
        api.DELETE("/v1/admin/plugins/{id}", {
          params: { path: { id }, query: force ? { force: true } : {} },
        }),
      ),
    onSuccess: () => qc.invalidateQueries({ queryKey: pluginKeys.all }),
  });
}

export function useHostClaims() {
  return useQuery({
    queryKey: pluginKeys.claims,
    queryFn: () => unwrap(api.GET("/v1/admin/host-claims")),
    retry,
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
    retry,
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
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (id: string) =>
      unwrap(api.DELETE("/v1/admin/trusted-keys/{id}", { params: { path: { id } } })),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["admin"] }),
  });
}
