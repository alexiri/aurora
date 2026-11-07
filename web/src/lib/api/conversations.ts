"use client";

// Conversation-plane calls use the generated client (`api.*` + `unwrap`). The sandbox, restart and
// agent-models routes are not in `schema.d.ts` yet, so they stay on the narrow fetch wrapper below.
// TODO(BE-051/052 follow-up): move `sandbox`, `restart`, `models` to `api.*` once the routes are generated.
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import { api, ApiError } from "./client";
import type { components } from "./schema";
import { unwrap } from "./unwrap";

export type Turn = components["schemas"]["Turn"];
export type Followup = components["schemas"]["Followup"];
export type Conversation = components["schemas"]["Conversation"];
// The schema types `payload` as `Record<string, never>`; transcript code reads arbitrary keys.
export type TranscriptEvent = Omit<components["schemas"]["TranscriptEvent"], "payload"> & {
  payload: Record<string, unknown>;
};
export type SendResult = components["schemas"]["MessageAccepted"];

// Not in the generated schema yet (wrapper routes below).
export interface SandboxView {
  agent_id: string;
  state: "unprovisioned" | "starting" | "running" | "stopping" | "stopped" | "failed";
  boot_id: string | null;
  runner_id: string | null;
  image: string | null;
  started_at: string | null;
  keep_awake_reasons: string[];
  failure_code: string | null;
  failure_message: string | null;
  processes: { id: string; name: string; state: string; busy: boolean; started_at: string | null }[];
  ports: { port: number; protocol: string }[];
  restart_to_update: boolean;
}
export interface ModelOption {
  endpoint_id: string;
  provider: string;
  model_id: string;
  display_name: string | null;
  dialects: string[];
}

async function call<T>(method: string, path: string, body?: unknown): Promise<T> {
  const res = await fetch(path, {
    method,
    credentials: "same-origin",
    headers: body === undefined ? undefined : { "content-type": "application/json" },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  const parsed: unknown = await res.json().catch(() => null);
  if (!res.ok) throw ApiError.fromBody(parsed, res.status);
  return parsed as T;
}

export const chatApi = {
  direct: (agent: string) => unwrap(api.POST("/v1/agents/{agent}/conversations/direct", { params: { path: { agent } } })),
  list: (agent: string) =>
    unwrap(api.GET("/v1/agents/{agent}/conversations", { params: { path: { agent }, query: { limit: 50 } } })),
  events: (id: string, afterSeq: number) =>
    unwrap(api.GET("/v1/conversations/{id}/events", { params: { path: { id }, query: { after_seq: afterSeq, limit: 500 } } })),
  send: (id: string, text: string, attachmentIds: string[] = []) =>
    unwrap(
      api.POST("/v1/conversations/{id}/messages", {
        params: { path: { id } },
        body: attachmentIds.length > 0 ? { text, attachment_ids: attachmentIds } : { text },
      }),
    ),
  continue: (id: string) => unwrap(api.POST("/v1/conversations/{id}/continue", { params: { path: { id } } })),
  abort: (turn: string) => unwrap(api.POST("/v1/turns/{id}/abort", { params: { path: { id: turn } } })),
  sendNow: (turn: string) => unwrap(api.POST("/v1/turns/{id}/send-now", { params: { path: { id: turn } } })),
  sandbox: (agent: string) => call<SandboxView>("GET", `/v1/agents/${agent}/sandbox`),
  restart: (agent: string) => call<SandboxView>("POST", `/v1/agents/${agent}/restart`),
  models: (agent: string) => call<{ items: ModelOption[] }>("GET", `/v1/agents/${agent}/models`),
};

export function useDirectConversation(agentId: string) {
  return useQuery({ queryKey: ["agents", agentId, "direct"] as const, queryFn: () => chatApi.direct(agentId), retry: false, staleTime: Infinity });
}
export function useConversations(agentId: string) {
  return useQuery({ queryKey: ["agents", agentId, "conversations"] as const, queryFn: () => chatApi.list(agentId), retry: false });
}
export function useSandbox(agentId: string) {
  return useQuery({
    queryKey: ["agents", agentId, "sandbox"] as const,
    queryFn: () => chatApi.sandbox(agentId),
    retry: false,
    refetchInterval: 5000,
  });
}
export function useRestartSandbox(agentId: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: () => chatApi.restart(agentId),
    onSuccess: (view) => qc.setQueryData(["agents", agentId, "sandbox"], view),
  });
}
export function useModels(agentId: string, enabled: boolean) {
  return useQuery({ queryKey: ["agents", agentId, "models"] as const, queryFn: () => chatApi.models(agentId), enabled, retry: false });
}
