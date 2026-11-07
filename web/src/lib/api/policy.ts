"use client";

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import { api, ApiError } from "./client";
import type { components } from "./schema";
import { unwrap } from "./unwrap";

type S = components["schemas"];
export type PolicyDocument = S["PolicyDocument"];
export type PolicyGeneration = S["PolicyGeneration"];
export type PolicyRule = S["PolicyRule"];
export type RuleInput = S["RuleInput"];
export type Target = S["Target"];
export type Identity = S["Identity"];
export type Condition = S["Condition"];
export type AllowlistEntry = S["AllowlistEntry"];
export type DraftRequest = S["DraftRequest"];
export type Simulation = S["Simulation"];
export type SimulateRequest = S["SimulateRequest"];
export type PolicyWarning = S["PolicyWarning"];
export type SecretGrant = S["SecretGrant"];
export type Effective = S["Effective"];

export type PolicyScope = { kind: "workspace" | "organization"; id: string };

export const policyKeys = {
  doc: (s: PolicyScope) => ["policy", s.kind, s.id] as const,
  warnings: (s: PolicyScope) => ["policy", s.kind, s.id, "warnings"] as const,
  generations: (s: PolicyScope) => ["policy", s.kind, s.id, "generations"] as const,
  grants: (agentId: string) => ["grants", agentId] as const,
  effective: (agentId: string) => ["effective", agentId] as const,
};

export function usePolicy(scope: PolicyScope) {
  return useQuery({
    queryKey: policyKeys.doc(scope),
    queryFn: () =>
      scope.kind === "workspace"
        ? unwrap(api.GET("/v1/workspaces/{ws}/policy", { params: { path: { ws: scope.id } } }))
        : unwrap(api.GET("/v1/orgs/{org}/policy", { params: { path: { org: scope.id } } })),
    retry: false,
  });
}

export function usePolicyWarnings(scope: PolicyScope) {
  return useQuery({
    queryKey: policyKeys.warnings(scope),
    queryFn: () =>
      scope.kind === "workspace"
        ? unwrap(api.GET("/v1/workspaces/{ws}/policy/warnings", { params: { path: { ws: scope.id } } }))
        : unwrap(api.GET("/v1/orgs/{org}/policy/warnings", { params: { path: { org: scope.id } } })),
    retry: false,
  });
}

export function usePolicyGenerations(scope: PolicyScope, enabled: boolean) {
  return useQuery({
    queryKey: policyKeys.generations(scope),
    enabled,
    queryFn: () =>
      scope.kind === "workspace"
        ? unwrap(api.GET("/v1/workspaces/{ws}/policy/generations", { params: { path: { ws: scope.id } } }))
        : unwrap(api.GET("/v1/orgs/{org}/policy/generations", { params: { path: { org: scope.id } } })),
    retry: false,
  });
}

function useInvalidatePolicy(scope: PolicyScope) {
  const qc = useQueryClient();
  return () => qc.invalidateQueries({ queryKey: ["policy", scope.kind, scope.id] });
}

export function usePutDraft(scope: PolicyScope) {
  const inv = useInvalidatePolicy(scope);
  return useMutation({
    mutationFn: (body: DraftRequest) =>
      scope.kind === "workspace"
        ? unwrap(api.PUT("/v1/workspaces/{ws}/policy/draft", { params: { path: { ws: scope.id } }, body }))
        : unwrap(api.PUT("/v1/orgs/{org}/policy/draft", { params: { path: { org: scope.id } }, body })),
    onSuccess: inv,
  });
}

export function usePublish(scope: PolicyScope) {
  const inv = useInvalidatePolicy(scope);
  return useMutation({
    mutationFn: (revision: number) =>
      scope.kind === "workspace"
        ? unwrap(api.POST("/v1/workspaces/{ws}/policy/publish", { params: { path: { ws: scope.id } }, body: { revision } }))
        : unwrap(api.POST("/v1/orgs/{org}/policy/publish", { params: { path: { org: scope.id } }, body: { revision } })),
    onSuccess: inv,
  });
}

export function useRollback(scope: PolicyScope) {
  const inv = useInvalidatePolicy(scope);
  return useMutation({
    mutationFn: (generation: number) =>
      scope.kind === "workspace"
        ? unwrap(api.POST("/v1/workspaces/{ws}/policy/rollback", { params: { path: { ws: scope.id } }, body: { generation } }))
        : unwrap(api.POST("/v1/orgs/{org}/policy/rollback", { params: { path: { org: scope.id } }, body: { generation } })),
    onSuccess: inv,
  });
}

export function useSimulate(wsId: string) {
  return useMutation({
    mutationFn: (body: SimulateRequest) =>
      unwrap(api.POST("/v1/workspaces/{ws}/policy/simulate", { params: { path: { ws: wsId } }, body })),
  });
}

export function useGrants(agentId: string) {
  return useQuery({
    queryKey: policyKeys.grants(agentId),
    queryFn: () => unwrap(api.GET("/v1/agents/{agent}/grants", { params: { path: { agent: agentId } } })),
    retry: false,
  });
}

export function useEffective(agentId: string) {
  return useQuery({
    queryKey: policyKeys.effective(agentId),
    queryFn: () => unwrap(api.GET("/v1/agents/{agent}/effective", { params: { path: { agent: agentId } } })),
    retry: false,
  });
}

export function useToggleGrant(agentId: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: async ({ secretId, grant }: { secretId: string; grant: boolean }) => {
      const params = { params: { path: { agent: agentId, secret: secretId } } };
      if (grant) return void (await unwrap(api.PUT("/v1/agents/{agent}/grants/secrets/{secret}", params)));
      const { error, response } = await api.DELETE("/v1/agents/{agent}/grants/secrets/{secret}", params);
      if (error !== undefined || !response.ok) throw ApiError.fromBody(error, response.status);
    },
    onSuccess: () => {
      void qc.invalidateQueries({ queryKey: policyKeys.grants(agentId) });
      void qc.invalidateQueries({ queryKey: policyKeys.effective(agentId) });
    },
  });
}

// ---- pure helpers (unit tested) ----

export interface DraftState {
  default_action: PolicyGeneration["default_action"];
  uncredentialed_default: PolicyGeneration["uncredentialed_default"];
  uncredentialed_allowlist: AllowlistEntry[];
  /** (RFE-003) workspace open-lane posture. */
  open_lane_posture: PolicyGeneration["open_lane_posture"];
  rules: RuleInput[];
}

export function ruleToInput(r: PolicyRule): RuleInput {
  return {
    logical_id: r.logical_id,
    name: r.name,
    description: r.description ?? null,
    enabled: r.enabled,
    identities: r.identities,
    targets: r.targets,
    action: r.action,
    require_approval: r.require_approval,
    rate_limit: r.rate_limit ?? null,
    conditions: r.conditions,
  };
}

export function draftFromGeneration(g: PolicyGeneration): DraftState {
  return {
    default_action: g.default_action,
    uncredentialed_default: g.uncredentialed_default,
    uncredentialed_allowlist: g.uncredentialed_allowlist.map((e) => ({ host: e.host, path: e.path ?? null })),
    open_lane_posture: g.open_lane_posture,
    rules: g.rules.map(ruleToInput),
  };
}

/** Modifiers (require approval, rate limit) exist only on allow rules. */
export function normalizeRule(r: RuleInput): RuleInput {
  const out: RuleInput = { ...r, description: r.description?.trim() ? r.description.trim() : null };
  if (out.action !== "allow") {
    out.require_approval = false;
    out.rate_limit = null;
  }
  return out;
}

export function toDraftRequest(s: DraftState, revision: number, scope: PolicyScope["kind"]): DraftRequest {
  const body: DraftRequest = {
    revision,
    default_action: s.default_action,
    rules: s.rules.map(normalizeRule),
  };
  if (scope === "workspace") {
    body.uncredentialed_default = s.uncredentialed_default;
    body.open_lane_posture = s.open_lane_posture;
    body.uncredentialed_allowlist = s.uncredentialed_allowlist
      .filter((e) => e.host.trim())
      .map((e) => (e.path?.trim() ? { host: e.host.trim(), path: e.path.trim() } : { host: e.host.trim() }));
  }
  return body;
}

export function moveRule<T>(rules: T[], from: number, to: number): T[] {
  if (to < 0 || to >= rules.length || from === to) return rules;
  const out = rules.slice();
  const [r] = out.splice(from, 1);
  out.splice(to, 0, r);
  return out;
}

export type TargetType = Target["type"];
export const TARGET_TYPES: { type: TargetType; label: string; disabled?: boolean }[] = [
  { type: "network", label: "Network host" },
  { type: "secret", label: "Secret" },
  { type: "secrets", label: "Secrets (by scope)" },
  { type: "app", label: "App (coming with apps)", disabled: true },
  { type: "connection", label: "Connection (coming with apps)", disabled: true },
  { type: "app_connections", label: "App connections (coming with apps)", disabled: true },
];

export function newTarget(type: TargetType, secretId = ""): Target {
  switch (type) {
    case "network":
      return { type, host: "" };
    case "secret":
      return { type, id: secretId };
    case "secrets":
      return { type, scope: "any" };
    case "app":
      return { type, app_id: "" };
    case "connection":
      return { type, id: "" };
    case "app_connections":
      return { type, app_id: "", scope: "any" };
  }
}

export function newRule(): RuleInput {
  return {
    name: "",
    description: null,
    enabled: true,
    identities: [],
    targets: [],
    action: "allow",
    require_approval: false,
    rate_limit: null,
    conditions: [],
  };
}

export function describeTarget(t: Target): string {
  switch (t.type) {
    case "network":
      return `${t.methods?.length ? t.methods.join(",") + " " : ""}${t.host}${t.path ?? ""}`;
    case "secret":
      return `secret ${t.id}`;
    case "secrets":
      return `${t.kind ?? "all"} secrets (${t.scope})`;
    case "app":
      return `app ${t.app_id}`;
    case "connection":
      return `connection ${t.id}`;
    case "app_connections":
      return `connections of ${t.app_id} (${t.scope})`;
  }
}

export function isStale(err: unknown): boolean {
  return err instanceof ApiError && err.code === "stale_revision";
}

export interface PolicyIssues {
  /** Messages per rule index. */
  rules: Record<number, string[]>;
  general: string[];
}

/** Maps `policy_invalid` `details.fields[]` (paths like `/rules/2/targets/0/host`) to rules. */
export function policyIssues(err: unknown): PolicyIssues {
  const out: PolicyIssues = { rules: {}, general: [] };
  if (!(err instanceof ApiError) || err.code !== "policy_invalid") return out;
  const d = err.details as { fields?: { path?: unknown; message?: unknown }[] } | null;
  const fields = Array.isArray(d?.fields) ? d.fields : [];
  if (fields.length === 0) out.general.push(err.message);
  for (const f of fields) {
    const path = typeof f.path === "string" ? f.path : "";
    const message = typeof f.message === "string" ? f.message : "Invalid value";
    const m = /(?:^|[/.])rules(?:[/.]|\[)(\d+)\]?(.*)$/.exec(path);
    if (m) {
      const idx = Number(m[1]);
      const field = m[2].replace(/^[/.]/, "");
      (out.rules[idx] ??= []).push(field ? `${field}: ${message}` : message);
    } else out.general.push(path ? `${path}: ${message}` : message);
  }
  return out;
}

const CREDENTIAL_TARGETS = new Set(["connection", "app_connections", "secret", "secrets"]);

/**
 * (phase 2, RFE-025) An enabled hand-written allow rule with empty identities that names
 * credentials applies to every agent of its scope, including agents created later
 * (`rule_applies_to_all_agents`, 07 §Injection selection). Mirrors the server warning so the
 * editor reacts to unsaved edits.
 */
export function appliesToAllAgents(r: RuleInput): boolean {
  return (
    r.enabled !== false &&
    r.action === "allow" &&
    (r.identities ?? []).length === 0 &&
    (r.targets ?? []).some((t) => CREDENTIAL_TARGETS.has(t.type))
  );
}

/** The warning text of the editor; `byoEnabled` adds the clause about members creating BYO agents. */
export function allAgentsText(byoEnabled: boolean): string {
  return `Applies to all agents, including agents created later${byoEnabled ? " and by members creating BYO agents" : ""}`;
}
