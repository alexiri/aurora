"use client";

// Conversation-plane calls use the generated client (`api.*` + `unwrap`). The sandbox, restart and
// agent-models routes are not in `schema.d.ts` yet, so they stay on the narrow fetch wrapper below.
// TODO(BE-051/052 follow-up): move `sandbox`, `restart`, `models` to `api.*` once the routes are generated.
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import { traceAction } from "@/lib/telemetry";

import { api, ApiError } from "./client";
import { fetchAllPages, PAGE_LIMIT } from "./paging";
import type { components } from "./schema";
import { unwrap } from "./unwrap";

export type Turn = components["schemas"]["Turn"];
export type Followup = components["schemas"]["Followup"];
export type Conversation = components["schemas"]["Conversation"];
/** (P5-REFINE-7, C-1074) The schedule or webhook a run thread belongs to. */
export type ConversationAutomation = components["schemas"]["ConversationAutomation"];
/** (P5-REFINE-7, C-1074) One automation folder of the conversation list. */
export type ConversationFolder = components["schemas"]["ConversationFolder"];
/** (P5-REFINE-7, C-1077) What a run or delivery came to. */
export type RunResult = "running" | "answered" | "skipped_by_gate" | "failed" | "skipped";
/**
 * (P5-REFINE-7, C-1079) `turn.queued.payload.display`: what people read for a schedule or webhook
 * turn (the turn's `text` is what the agent receives; old events have no `display`).
 */
export type TurnQueuedDisplay =
  | { kind: "schedule"; name: string; schedule_id: string; run_id: string }
  | {
      kind: "webhook";
      name: string;
      delivery_id: string;
      content_type: string;
      size: number;
      truncated: boolean;
      body: string;
    };
/** (P5-REFINE-7, C-1074) Which run threads a conversation list includes. */
export type AutomationFilter = "all" | "none" | "only";
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
/** (P5-REFINE-4, C-1059, D-408, D-412) The computer in words, for everyone who can use the agent. */
export interface ComputerView {
  word: "off" | "starting" | "on" | "stopping" | "failed";
  /** The computer answers: running, the supervisor connected and the harness reported ready (the header's Ready). */
  ready: boolean;
  /** A turn or a busy process holds it awake. */
  working: boolean;
  /** Stopped or never started, with no failure (the header's Asleep). */
  asleep: boolean;
  /** The raw start phase (fixed vocabulary), shown at Expert as the secondary line. */
  phase: string | null;
  /** Seconds until the idle stop while on and idle; null otherwise (a hint). */
  sleeps_in_secs: number | null;
  started_at: string | null;
}
export interface SandboxView {
  agent_id: string;
  state: "unprovisioned" | "starting" | "running" | "stopping" | "stopped" | "failed";
  /** Changes when the computer restarts (C-1058: the terminal's restart notice compares it). */
  boot_id: string | null;
  /** (SEC-141) Instance admins only; null for everyone else. */
  runner_id: string | null;
  /** (SEC-141) Instance admins only; null for everyone else. */
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
  /** (P5-APIGAP-1, C-1046) Seconds of quiet after which this computer sleeps; null when idle stop is off. */
  idle_stop_after_secs?: number | null;
  /** (P5-REFINE-4, C-1059) The computer in words. */
  computer: ComputerView;
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
  /** (P5-REFINE-7, C-1073) A fresh direct chat: 201 a new one, 200 the latest while it is still empty. */
  newDirect: (agent: string) => unwrap(api.POST("/v1/agents/{agent}/conversations/direct/new", { params: { path: { agent } } })),
  /** (P5-REFINE-7, C-1073) Rename; `null` goes back to the automatic title. 409 `title_managed` for run threads. */
  rename: (id: string, title: string | null) => unwrap(api.PATCH("/v1/conversations/{id}", { params: { path: { id } }, body: { title } })),
  list: (
    agent: string,
    archived = false,
    opts: { automation?: AutomationFilter; automationId?: string; cursor?: string } = {},
  ) =>
    unwrap(
      api.GET("/v1/agents/{agent}/conversations", {
        params: {
          path: { agent },
          query: {
            limit: 50,
            archived,
            ...(opts.automation ? { automation: opts.automation } : {}),
            ...(opts.automationId ? { automation_id: opts.automationId } : {}),
            ...(opts.cursor ? { cursor: opts.cursor } : {}),
          },
        },
      }),
    ),
  /** (P5-REFINE-7, C-1074) The agent's automation folders, newest activity first. */
  listFolders: (agent: string, cursor?: string) =>
    unwrap(
      api.GET("/v1/agents/{agent}/conversation-folders", {
        params: { path: { agent }, query: { limit: PAGE_LIMIT, ...(cursor ? { cursor } : {}) } },
      }),
    ),
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
  /** `replyToSeq`: quote an earlier message of this conversation (C-725); `text` may be empty with attachments (C-726). */
  send: (id: string, text: string, attachmentIds: string[] = [], idempotencyKey?: string, replyToSeq?: number) =>
    traceAction("send_message", () =>
      unwrap(
        api.POST("/v1/conversations/{id}/messages", {
          params: { path: { id }, ...(idempotencyKey ? { header: { "Idempotency-Key": idempotencyKey } } : {}) },
          body: {
            text,
            ...(attachmentIds.length > 0 ? { attachment_ids: attachmentIds } : {}),
            ...(replyToSeq ? { reply_to: { seq: replyToSeq } } : {}),
          },
        }),
      ),
    ),
  continue: (id: string) => unwrap(api.POST("/v1/conversations/{id}/continue", { params: { path: { id } } })),
  abort: (turn: string) => unwrap(api.POST("/v1/turns/{id}/abort", { params: { path: { id: turn } } })),
  sendNow: (turn: string) => unwrap(api.POST("/v1/turns/{id}/send-now", { params: { path: { id: turn } } })),
  sandbox: (agent: string) => call<SandboxView>("GET", `/v1/agents/${agent}/sandbox`),
  restart: (agent: string) => call<SandboxView>("POST", `/v1/agents/${agent}/restart`),
  /** (FIX-029, C-610) Retry a failed start: 202 → the parked sandbox view. */
  retry: (agent: string) => call<SandboxView>("POST", `/v1/agents/${agent}/sandbox/retry`),
  /** (P5-APIGAP-1, C-1045) Wake a sleeping computer: 202 (start requested) or 200 (already awake) with the view. */
  wake: (agent: string) => call<SandboxView>("POST", `/v1/agents/${agent}/sandbox/wake`),
  /** (P5-REFINE-4, C-1060) Sleep now: 202 (stop requested) or 200 (already asleep) with the view; 409 `sandbox_busy` while work runs. */
  sleep: (agent: string) => call<SandboxView>("POST", `/v1/agents/${agent}/sandbox/sleep`),
  /** (FIX-029, C-613) The agent's recent start failures, newest first (≤ 20). */
  startFailures: (agent: string) => unwrap(api.GET("/v1/agents/{agent}/start-failures", { params: { path: { agent } } })),
  models: (agent: string) => call<{ items: ModelOption[] }>("GET", `/v1/agents/${agent}/models`),
};

export function useDirectConversation(agentId: string) {
  return useQuery({ queryKey: ["agents", agentId, "direct"] as const, queryFn: () => chatApi.direct(agentId), retry: false, staleTime: Infinity });
}
/**
 * `automation` (C-1074): `all` (default) includes the run threads of schedules and webhooks, `none` leaves them out (the
 * list the person reads; they sit in folders), `only` with `automationId` is one folder's threads.
 */
export function useConversations(agentId: string, archived = false, opts: { automation?: AutomationFilter; automationId?: string; enabled?: boolean } = {}) {
  const { automation, automationId, enabled = true } = opts;
  return useQuery({
    // The first three parts are the prefix `refreshConversations` and the 15 s poll invalidate.
    queryKey: ["agents", agentId, "conversations", archived, ...(automation ? [automation, automationId ?? ""] : [])] as const,
    // Every page (FIX-117): the first 50 are not the whole list, and an older conversation must still resolve.
    queryFn: () =>
      fetchAllPages((cursor) =>
        unwrap(
          api.GET("/v1/agents/{agent}/conversations", {
            params: {
              path: { agent: agentId },
              query: { limit: PAGE_LIMIT, archived, cursor, ...(automation ? { automation } : {}), ...(automationId ? { automation_id: automationId } : {}) },
            },
          }),
        ),
      ),
    enabled,
    retry: false,
  });
}

/** (D-409) A fresh direct chat. */
export function useNewDirectChat(agentId: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: () => chatApi.newDirect(agentId),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["agents", agentId, "conversations"] }),
  });
}

/** (D-409, C-1073) Rename a conversation; `null` goes back to the automatic title. */
export function useRenameConversation(agentId: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({ id, title }: { id: string; title: string | null }) => chatApi.rename(id, title),
    onSuccess: (_c, v) => {
      void qc.invalidateQueries({ queryKey: ["agents", agentId, "conversations"] });
      void qc.invalidateQueries({ queryKey: ["conversations", v.id, "one"] });
    },
  });
}

/** (P5-REFINE-7, C-1074) Every automation folder of the agent (all pages). */
export function useConversationFolders(agentId: string, enabled = true) {
  return useQuery({
    queryKey: ["agents", agentId, "conversation-folders"] as const,
    queryFn: () => fetchAllPages((cursor) => chatApi.listFolders(agentId, cursor)),
    enabled,
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
export const exportUrl = (id: string, kind: "json" | "zip") => `/v1/conversations/${id}/export${kind === "zip" ? "?attachments=zip" : ""}`;
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
/** (P5-APIGAP-1, C-1045) **Wake it now**: the view answers `starting` once the start is claimed; the 5 s poll follows it. */
export function useWakeSandbox(agentId: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: () => chatApi.wake(agentId),
    onSuccess: (view) => qc.setQueryData(["agents", agentId, "sandbox"], view),
  });
}
/** (P5-REFINE-15, C-1060) **Sleep now**: 202 with the view while it stops; 409 `sandbox_busy` while work runs. */
export function useSleepSandbox(agentId: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: () => chatApi.sleep(agentId),
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
