"use client";

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import { api, ApiError } from "./client";
import { policyKeys } from "./policy";
import type { components } from "./schema";
import { unwrap } from "./unwrap";

type S = components["schemas"];
export type Account = S["Account"];
export type AgentAccess = S["AgentAccess"];
export type PutConnectionAccess = S["PutConnectionAccess"];
export type Level = "read" | "ask" | "act";

/** Where the list is read: a workspace (optionally for one agent) or the organization. */
export type AccountsScope = { kind: "workspace"; id: string; agentId?: string } | { kind: "organization"; id: string };

export const accountKeys = {
  all: ["accounts"] as const,
  list: (s: AccountsScope) => ["accounts", s.kind, s.id, s.kind === "workspace" ? (s.agentId ?? null) : null] as const,
};

/** C-917: one list of what agents can use. */
export function useAccounts(scope: AccountsScope, enabled = true) {
  return useQuery({
    queryKey: accountKeys.list(scope),
    enabled,
    queryFn: () =>
      scope.kind === "workspace"
        ? unwrap(
            api.GET("/v1/workspaces/{ws}/accounts", {
              params: { path: { ws: scope.id }, query: scope.agentId ? { agent_id: scope.agentId } : {} },
            }),
          )
        : unwrap(api.GET("/v1/orgs/{org}/accounts", { params: { path: { org: scope.id } } })),
    retry: false,
  });
}

export type TestAnswer = S["AccountTestAnswer"];

/** C-943: check one account now. The result is stored by the server (except `not_testable`), so the lists refresh. */
export function useTestAccount() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({ ref }: { ref: Account["ref"] }) =>
      unwrap(api.POST("/v1/accounts/{type}/{id}/test", { params: { path: { type: ref.type, id: ref.id } } })),
    onSuccess: () => void qc.invalidateQueries({ queryKey: accountKeys.all }),
  });
}

/** Anything that changed an agent's access to an account refreshes the lists and the agent's grant views. */
function useAccessInvalidation() {
  const qc = useQueryClient();
  return (agentId: string) => {
    void qc.invalidateQueries({ queryKey: accountKeys.all });
    void qc.invalidateQueries({ queryKey: ["connections"] });
    void qc.invalidateQueries({ queryKey: policyKeys.grants(agentId) });
    void qc.invalidateQueries({ queryKey: policyKeys.effective(agentId) });
    void qc.invalidateQueries({ queryKey: ["model-readiness", agentId] });
  };
}

/** C-980: one save applies an agent's access to one app account (a level, or custom with tool lists). */
export function usePutAccess() {
  const done = useAccessInvalidation();
  return useMutation({
    mutationFn: ({ agentId, connectionId, body }: { agentId: string; connectionId: string; body: PutConnectionAccess }) =>
      unwrap(api.PUT("/v1/agents/{agent}/grants/connections/{conn}", { params: { path: { agent: agentId, conn: connectionId } }, body })),
    onSuccess: (_r, v) => done(v.agentId),
  });
}

/** Give an agent a website or an AI key (level `use`). */
export function useGiveSecret() {
  const done = useAccessInvalidation();
  return useMutation({
    mutationFn: ({ agentId, secretId }: { agentId: string; secretId: string }) =>
      unwrap(api.PUT("/v1/agents/{agent}/grants/secrets/{secret}", { params: { path: { agent: agentId, secret: secretId } } })),
    onSuccess: (_r, v) => done(v.agentId),
  });
}

/** Remove one agent's access to an account. */
export function useRemoveAccess() {
  const done = useAccessInvalidation();
  return useMutation({
    mutationFn: async ({ agentId, account }: { agentId: string; account: Pick<Account, "ref"> }) => {
      const r =
        account.ref.type === "connection"
          ? await api.DELETE("/v1/agents/{agent}/grants/connections/{conn}", { params: { path: { agent: agentId, conn: account.ref.id } } })
          : await api.DELETE("/v1/agents/{agent}/grants/secrets/{secret}", { params: { path: { agent: agentId, secret: account.ref.id } } });
      if (r.error !== undefined || !r.response.ok) throw ApiError.fromBody(r.error, r.response.status);
    },
    onSuccess: (_r, v) => done(v.agentId),
  });
}
