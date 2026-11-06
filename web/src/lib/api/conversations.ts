"use client";

// TODO(BE-051/052): the M4 routes are not in the generated schema yet. These narrowly
// typed fetch wrappers replace `api.*` calls until `schema.d.ts` is regenerated (D-32).
// Calls using the wrapper: every function in this file.
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import { ApiError } from "./client";

export type TurnStatus = "queued" | "dispatched" | "running" | "done" | "failed" | "aborted";
export interface Turn {
  id: string;
  conversation_id: string;
  status: TurnStatus;
  door: string;
  speaker_label: string | null;
  text: string;
  attachment_ids: string[];
  error_code: string | null;
  abort_requested: boolean;
  send_now_requested: boolean;
  created_at: string;
  started_at: string | null;
  finished_at: string | null;
}
export interface Followup {
  id: string;
  parent_turn_id: string;
  status: "joining" | "joined" | "promoted";
  text: string;
  created_at: string;
}
export interface Conversation {
  id: string;
  agent_id: string;
  kind: "direct" | "group" | "schedule" | "webhook" | "watch" | "peer" | "api";
  external_ref: string;
  title: string | null;
  last_seq: number;
  active_turn: Turn | null;
  last_activity_at: string | null;
  created_at: string;
}
export interface TranscriptEvent {
  seq: number | null;
  type: string;
  ts: string;
  payload: Record<string, unknown>;
}
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
export interface SendResult {
  turn: Turn | null;
  followup: Followup | null;
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
  direct: (agent: string) => call<Conversation>("POST", `/v1/agents/${agent}/conversations/direct`),
  list: (agent: string) => call<{ items: Conversation[]; next_cursor: string | null }>("GET", `/v1/agents/${agent}/conversations?limit=50`),
  events: (id: string, afterSeq: number) =>
    call<{ items: TranscriptEvent[]; last_seq: number }>("GET", `/v1/conversations/${id}/events?after_seq=${afterSeq}&limit=500`),
  send: (id: string, text: string) => call<SendResult>("POST", `/v1/conversations/${id}/messages`, { text }),
  abort: (turn: string) => call<Turn>("POST", `/v1/turns/${turn}/abort`),
  sendNow: (turn: string) => call<Turn>("POST", `/v1/turns/${turn}/send-now`),
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
