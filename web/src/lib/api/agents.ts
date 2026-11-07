"use client";

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import { api, ApiError } from "./client";
import { queryKeys, type Agent } from "./queries";
import type { components } from "./schema";
import { unwrap } from "./unwrap";

export type CreateAgentRequest = components["schemas"]["CreateAgentRequest"];
export type PatchAgentRequest = components["schemas"]["PatchAgentRequest"];
export type ContainerConfig = components["schemas"]["ContainerConfig"];

export function useCreateAgent(wsId: string | undefined) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (body: CreateAgentRequest) =>
      unwrap(api.POST("/v1/workspaces/{ws}/agents", { params: { path: { ws: wsId! } }, body })),
    onSuccess: () => qc.invalidateQueries({ queryKey: queryKeys.agents(wsId ?? "") }),
  });
}

export function usePatchAgent(wsId: string | undefined, agentId: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (body: PatchAgentRequest) =>
      unwrap(api.PATCH("/v1/agents/{agent}", { params: { path: { agent: agentId } }, body })),
    onSuccess: () => qc.invalidateQueries({ queryKey: queryKeys.agents(wsId ?? "") }),
  });
}

export type PutByoRequest = components["schemas"]["PutByoRequest"];

/** (phase 2, RFE-025) The agent's own source list and require-context flag (`can_own`). */
export function usePutAgentByo(wsId: string | undefined, agentId: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (body: PutByoRequest) =>
      unwrap(api.PUT("/v1/agents/{agent}/byo", { params: { path: { agent: agentId } }, body })),
    onSuccess: () => {
      void qc.invalidateQueries({ queryKey: queryKeys.agents(wsId ?? "") });
      void qc.invalidateQueries({ queryKey: ["agents", agentId, "container-config"] });
    },
  });
}

export function useDeleteAgent(wsId: string | undefined, agentId: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: async () => {
      const { error, response } = await api.DELETE("/v1/agents/{agent}", {
        params: { path: { agent: agentId } },
      });
      if (error !== undefined || !response.ok) throw ApiError.fromBody(error, response.status);
    },
    onSuccess: () => qc.invalidateQueries({ queryKey: queryKeys.agents(wsId ?? "") }),
  });
}

/** Placeholder config only; the real token is never fetched through this. */
export function useContainerConfig(agentId: string, enabled: boolean) {
  return useQuery({
    queryKey: ["agents", agentId, "container-config"] as const,
    queryFn: () =>
      unwrap(
        api.GET("/v1/agents/{agent}/container-config", { params: { path: { agent: agentId } } }),
      ),
    enabled,
    retry: false,
  });
}

/** The response carries the token; the caller keeps it in component state only. */
export function useRegenerateToken(agentId: string) {
  return useMutation({
    mutationFn: () =>
      unwrap(api.POST("/v1/agents/{agent}/token", { params: { path: { agent: agentId } } })),
    gcTime: 0,
  });
}

export type { Agent };

/** `GET /v1/gateway/ca.pem` availability probe; 503 `gateway_ca_unavailable` yields an error. */
export async function probeGatewayCa(): Promise<void> {
  const res = await fetch("/v1/gateway/ca.pem", { credentials: "same-origin" });
  if (!res.ok) throw ApiError.fromBody(await res.json().catch(() => null), res.status);
}

export function agentSnippets(handle: string, config: ContainerConfig | undefined) {
  const envLines = config
    ? [
        `export HTTPS_PROXY='${config.proxy_url}'`,
        `export HTTP_PROXY='${config.proxy_url}'`,
        ...Object.entries(config.env).map(([k, v]) => `export ${k}='${v}'`),
      ].join("\n")
    : "";
  return {
    login: "aurora login",
    run: `aurora run --agent ${handle} -- <cmd>`,
    env: envLines,
  };
}
