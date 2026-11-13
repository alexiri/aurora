"use client";

// Conversation-plane calls use the generated client (`api.*` + `unwrap`). The sandbox, restart and
// agent-models routes are not in `schema.d.ts` yet, so they stay on the narrow fetch wrapper below.
// TODO(BE-051/052 follow-up): move `sandbox`, `restart`, `models` to `api.*` once the routes are generated.
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import { api, ApiError } from "./client";
import { fetchAllPages, PAGE_LIMIT } from "./paging";
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
export type StartLogEntry = components["schemas"]["StartLogEntry"];
export type AgentStartFailure = components["schemas"]["AgentStartFailure"];

// Not in the generated schema yet (wrapper routes below).
export interface SandboxStartAttempt {
  started_at: string;
  phase: string | null;
  detail: string | null;
  events: string[];
  progress_at: string | null;
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
  start_attempt: SandboxStartAttempt | null;
  /** (FIX-029, C-611) The failed start may be retried (state `failed`). */
  retryable: boolean;
  /** (FIX-029, C-611) The boot's start log, newest first (C-612). */
  start_log: StartLogEntry[];
  processes: { id: string; name: string; state: string; busy: boolean; started_at: string | null }[];
  ports: { port: number; protocol: string; shared?: string | null }[];
  // (P4-PRV-1, D-145) `preview` is among `keep_awake_reasons` while this is true.
  preview_in_use?: boolean;
  // (P4-TERM, C-621) `terminal` is among `keep_awake_reasons` while this is true.
  terminal_in_use?: boolean;
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
  list: (agent: string, archived = false) =>
    unwrap(api.GET("/v1/agents/{agent}/conversations", { params: { path: { agent }, query: { limit: 50, archived } } })),
  archive: (id: string) => unwrap(api.POST("/v1/conversations/{id}/archive", { params: { path: { id } } })),
  unarchive: (id: string) => unwrap(api.POST("/v1/conversations/{id}/unarchive", { params: { path: { id } } })),
  remove: async (id: string) => {
    const { error, response } = await api.DELETE("/v1/conversations/{id}", { params: { path: { id } } });
    if (error !== undefined || !response.ok) throw ApiError.fromBody(error, response.status);
  },
  redact: (id: string, seq: number, body: { reason?: string; reset_session?: boolean }) =>
    unwrap(api.POST("/v1/conversations/{id}/events/{seq}/redact", { params: { path: { id, seq } }, body })),
  events: (id: string, afterSeq: number) =>
    unwrap(api.GET("/v1/conversations/{id}/events", { params: { path: { id }, query: { after_seq: afterSeq, limit: 500 } } })),
  /** `idempotencyKey` (RFE-018): the same key on a retry returns the first result instead of a second message. */
  send: (id: string, text: string, attachmentIds: string[] = [], idempotencyKey?: string) =>
    unwrap(
      api.POST("/v1/conversations/{id}/messages", {
        params: { path: { id }, ...(idempotencyKey ? { header: { "Idempotency-Key": idempotencyKey } } : {}) },
        body: attachmentIds.length > 0 ? { text, attachment_ids: attachmentIds } : { text },
      }),
    ),
  continue: (id: string) => unwrap(api.POST("/v1/conversations/{id}/continue", { params: { path: { id } } })),
  abort: (turn: string) => unwrap(api.POST("/v1/turns/{id}/abort", { params: { path: { id: turn } } })),
  sendNow: (turn: string) => unwrap(api.POST("/v1/turns/{id}/send-now", { params: { path: { id: turn } } })),
  sandbox: (agent: string) => call<SandboxView>("GET", `/v1/agents/${agent}/sandbox`),
  restart: (agent: string) => call<SandboxView>("POST", `/v1/agents/${agent}/restart`),
  /** (FIX-029, C-610) Retry a failed start: 202 → the parked sandbox view. */
  retry: (agent: string) => call<SandboxView>("POST", `/v1/agents/${agent}/sandbox/retry`),
  /** (FIX-029, C-613) The agent's recent start failures, newest first (≤ 20). */
  startFailures: (agent: string) => unwrap(api.GET("/v1/agents/{agent}/start-failures", { params: { path: { agent } } })),
  models: (agent: string) => call<{ items: ModelOption[] }>("GET", `/v1/agents/${agent}/models`),
};

export function useDirectConversation(agentId: string) {
  return useQuery({ queryKey: ["agents", agentId, "direct"] as const, queryFn: () => chatApi.direct(agentId), retry: false, staleTime: Infinity });
}
export function useConversations(agentId: string, archived = false) {
  return useQuery({
    queryKey: ["agents", agentId, "conversations", archived] as const,
    // Every page (FIX-117): the first 50 are not the whole list, and an older conversation must still resolve.
    queryFn: () =>
      fetchAllPages((cursor) =>
        unwrap(
          api.GET("/v1/agents/{agent}/conversations", {
            params: { path: { agent: agentId }, query: { limit: PAGE_LIMIT, archived, cursor } },
          }),
        ),
      ),
    retry: false,
  });
}

/** One conversation by id: a deep link to one that is not among the loaded pages still gets its title and kind. */
export function useConversationById(id: string | null, enabled: boolean) {
  return useQuery({
    queryKey: ["conversations", id ?? "", "one"] as const,
    queryFn: () => unwrap(api.GET("/v1/conversations/{id}", { params: { path: { id: id! } } })),
    enabled: enabled && !!id,
    retry: false,
  });
}

/** Download URL of a conversation export (`aurora.conversation.v1` JSON, or a zip with the files). */
export const exportUrl = (id: string, kind: "json" | "zip") =>
  `/v1/conversations/${id}/export${kind === "zip" ? "?attachments=zip" : ""}`;
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
/** (FIX-029, C-610) Retry a failed start; the 202 view parks the sandbox back in `stopped`. */
export function useRetrySandbox(agentId: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: () => chatApi.retry(agentId),
    onSuccess: (view) => {
      qc.setQueryData(["agents", agentId, "sandbox"], view);
      // The start arm claims the retry on the next runner poll: refetch to follow the new boot.
      void qc.invalidateQueries({ queryKey: ["agents", agentId, "start-failures"] });
    },
  });
}
/** (FIX-029, C-613) The agent's recent start failures (newest first, ≤ 20). */
export function useStartFailures(agentId: string, enabled = true) {
  return useQuery({
    queryKey: ["agents", agentId, "start-failures"] as const,
    queryFn: () => chatApi.startFailures(agentId),
    enabled,
    retry: false,
  });
}
export function useModels(agentId: string, enabled: boolean) {
  return useQuery({ queryKey: ["agents", agentId, "models"] as const, queryFn: () => chatApi.models(agentId), enabled, retry: false });
}
