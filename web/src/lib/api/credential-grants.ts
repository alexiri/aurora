"use client";

import { useMutation, useQuery, useQueryClient, type QueryClient } from "@tanstack/react-query";

import { api, ApiError } from "./client";
import { policyKeys } from "./policy";
import type { components } from "./schema";
import { unwrap } from "./unwrap";

type S = components["schemas"];
export type ModelReadiness = S["ModelReadiness"];
export type KeyReadiness = S["KeyReadiness"];

/** What a credential-side grant is about. */
export type GrantTarget = { kind: "secret" | "connection"; id: string };

export const grantKeys = {
  secret: (id: string) => ["credential-grants", "secret", id] as const,
  connection: (id: string) => ["credential-grants", "connection", id] as const,
  readiness: (agentId: string) => ["model-readiness", agentId] as const,
};

const listKey = (t: GrantTarget) => (t.kind === "secret" ? grantKeys.secret(t.id) : grantKeys.connection(t.id));

/** Grants changed: the credential's own list, every touched agent's grants/effective view and the model readiness. */
function invalidateGrants(qc: QueryClient, target: GrantTarget, agentIds: string[]) {
  void qc.invalidateQueries({ queryKey: listKey(target) });
  for (const a of agentIds) {
    void qc.invalidateQueries({ queryKey: policyKeys.grants(a) });
    void qc.invalidateQueries({ queryKey: policyKeys.effective(a) });
    void qc.invalidateQueries({ queryKey: grantKeys.readiness(a) });
  }
}

/** Per-key readiness and the default model of a hosted agent (C-500; owners only). */
export function useModelReadiness(agentId: string, enabled = true) {
  return useQuery({
    queryKey: grantKeys.readiness(agentId),
    enabled,
    queryFn: () => unwrap(api.GET("/v1/agents/{agent}/model-readiness", { params: { path: { agent: agentId } } })),
    retry: false,
  });
}

/** An agent that holds a secret or a connection (`GET …/grants`, C-503/C-504), one row shape for both. */
export interface GrantedAgent {
  agent_id: string;
  agent_name: string;
  workspace_id: string;
  workspace_name: string;
}

export function useCredentialGrants(target: GrantTarget, enabled = true) {
  return useQuery({
    queryKey: listKey(target),
    enabled,
    retry: false,
    queryFn: async (): Promise<GrantedAgent[]> => {
      const items =
        target.kind === "secret"
          ? (await unwrap(api.GET("/v1/secrets/{id}/grants", { params: { path: { id: target.id } } }))).items
          : (await unwrap(api.GET("/v1/connections/{id}/grants", { params: { path: { id: target.id } } }))).items;
      return items.map((g) => ({
        agent_id: g.agent_id,
        agent_name: g.agent_name,
        workspace_id: g.workspace_id,
        workspace_name: g.workspace_name,
      }));
    },
  });
}

export interface AddGrantsInput {
  agentIds: string[];
  /** Connections only: a custom grant (every other tool blocked). Absent = `full`. */
  custom?: { allow_tools: string[] };
}

export interface AddGrantsResult {
  created: string[];
  existing: string[];
}

/** Bulk add (C-503, C-504). All-or-nothing: a failure carries `details.agent_id`. */
export function useAddCredentialGrants(target: GrantTarget) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: async ({ agentIds, custom }: AddGrantsInput): Promise<AddGrantsResult> => {
      const items =
        target.kind === "secret"
          ? (
              await unwrap(
                api.POST("/v1/secrets/{id}/grants", { params: { path: { id: target.id } }, body: { agent_ids: agentIds } }),
              )
            ).items
          : (
              await unwrap(
                api.POST("/v1/connections/{id}/grants", {
                  params: { path: { id: target.id } },
                  body: custom
                    ? { agent_ids: agentIds, mode: "custom", allow_tools: custom.allow_tools, ask_tools: [] }
                    : { agent_ids: agentIds, mode: "full" },
                }),
              )
            ).items;
      return {
        created: items.filter((i) => i.status === "created").map((i) => i.agent_id),
        existing: items.filter((i) => i.status === "existing").map((i) => i.agent_id),
      };
    },
    onSuccess: (_r, v) => invalidateGrants(qc, target, v.agentIds),
  });
}

/** Removal stays the agent-side `DELETE` (D-137). */
export function useRemoveCredentialGrant(target: GrantTarget) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: async (agentId: string) => {
      const { error, response } =
        target.kind === "secret"
          ? await api.DELETE("/v1/agents/{agent}/grants/secrets/{secret}", { params: { path: { agent: agentId, secret: target.id } } })
          : await api.DELETE("/v1/agents/{agent}/grants/connections/{conn}", { params: { path: { agent: agentId, conn: target.id } } });
      if (error !== undefined || !response.ok) throw ApiError.fromBody(error, response.status);
    },
    onSuccess: (_r, agentId) => invalidateGrants(qc, target, [agentId]),
  });
}

/** The agent id a failed bulk grant names in `details.agent_id`, if any. */
export function failingAgentId(err: unknown): string | null {
  if (!(err instanceof ApiError)) return null;
  const d = err.details as { agent_id?: unknown } | null;
  return typeof d?.agent_id === "string" ? d.agent_id : null;
}
