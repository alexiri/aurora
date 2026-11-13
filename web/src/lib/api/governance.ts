"use client";

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import { api, ApiError } from "./client";
import { fetchAllPages } from "./paging";
import type { components } from "./schema";
import { unwrap } from "./unwrap";

type S = components["schemas"];
export type EffectiveGovernance = S["EffectiveGovernance"];
export type EffectiveEndpoint = S["EffectiveEndpoint"];
export type InstanceGovernance = S["InstanceGovernance"];
export type AllowlistEntry = S["AllowlistEntry"];

export type KeyScope = "organization" | "workspace";
export const KEY_SCOPES: KeyScope[] = ["organization", "workspace"];

/** The `08 §11.2` organization governance document (generated schema, D-32). */
export type GovDocument = S["OrgGovernance"];
export type GovEndpointDoc = S["OrgEndpoint"];

export const govKeys = {
  doc: (org: string) => ["governance", org, "doc"] as const,
  effective: (org: string) => ["governance", org, "effective"] as const,
  instance: ["governance", "instance"] as const,
  blocked: (org: string) => ["governance", org, "blocked"] as const,
};

const retry = (count: number, err: unknown) =>
  !(err instanceof ApiError && err.status !== undefined && err.status >= 400 && err.status < 500) && count < 1;

export function useGovernanceDoc(orgId: string, enabled: boolean) {
  return useQuery({
    queryKey: govKeys.doc(orgId),
    enabled,
    queryFn: () => unwrap(api.GET("/v1/orgs/{org}/governance", { params: { path: { org: orgId } } })),
    retry,
  });
}

export function useEffectiveGovernance(orgId: string) {
  return useQuery({
    queryKey: govKeys.effective(orgId),
    queryFn: () => unwrap(api.GET("/v1/orgs/{org}/governance/effective", { params: { path: { org: orgId } } })),
    retry,
  });
}

export function useInstanceGovernance(enabled: boolean) {
  return useQuery({
    queryKey: govKeys.instance,
    enabled,
    queryFn: () => unwrap(api.GET("/v1/governance/instance")),
    retry,
  });
}

export function usePutGovernance(orgId: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (body: { document: GovDocument; revision: number }) =>
      unwrap(
        api.PUT("/v1/orgs/{org}/governance", {
          params: { path: { org: orgId } },
          body: { document: body.document, revision: body.revision },
        }),
      ),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["governance", orgId] }),
  });
}

export interface BlockedHost {
  host: string;
  count: number;
}

/** Activity pages read per workspace (100 rows each): the count is of *recent* refusals, not an audit. */
const BLOCKED_PAGES_PER_WORKSPACE = 3;
/** Workspaces read at once (the count aggregates all of them; no API count exists yet, so this is client-side). */
const BLOCKED_CONCURRENCY = 4;

/**
 * Hosts recently refused by egress policy, aggregated over **every** workspace the viewer can see (FIX-117; the
 * API has no count, so each workspace's most recent refusals are read, a few pages deep, a few workspaces at a time).
 */
export function useBlockedHosts(workspaceIds: string[], enabled: boolean): BlockedHost[] {
  const q = useQuery({
    queryKey: ["governance", "blocked", workspaceIds] as const,
    enabled: enabled && workspaceIds.length > 0,
    retry: false,
    queryFn: async () => {
      const rows: { host: string }[] = [];
      for (let i = 0; i < workspaceIds.length; i += BLOCKED_CONCURRENCY) {
        const batch = workspaceIds.slice(i, i + BLOCKED_CONCURRENCY);
        const pages = await Promise.all(
          batch.map((ws) =>
            fetchAllPages(
              (cursor) =>
                unwrap(
                  api.GET("/v1/workspaces/{ws}/activity", {
                    params: { path: { ws }, query: { decision: "egress_denied", limit: 100, cursor } },
                  }),
                ),
              BLOCKED_PAGES_PER_WORKSPACE,
            ),
          ),
        );
        for (const p of pages) rows.push(...p.items);
      }
      return rows;
    },
  });
  return countBlockedHosts(q.data ?? []);
}

export function countBlockedHosts(items: { host: string }[]): BlockedHost[] {
  const m = new Map<string, number>();
  for (const i of items) m.set(i.host, (m.get(i.host) ?? 0) + 1);
  return [...m.entries()].map(([host, count]) => ({ host, count })).sort((a, b) => b.count - a.count || a.host.localeCompare(b.host));
}

// ---- pure helpers (unit tested) ----

export interface EndpointForm {
  id: string;
  provider: string;
  display_name: string;
  allowed: boolean;
  key_scopes: string[];
  /** One per line / comma separated. */
  allowed_models: string;
  org_ids: string;
  workspace_ids: string;
  team_ids: string;
  locked: { permitted: boolean; key_scopes: boolean; allowed_models: boolean };
}
export type OpenLane = "enabled" | "allowlist_only" | "disabled";

/** (phase 2, RFE-025) BYO posture, ordered by strictness. */
export type ByoPosture = "enabled" | "owners_only" | "disabled";
export const BYO_POSTURES: ByoPosture[] = ["enabled", "owners_only", "disabled"];
export const byoRank = (p: string): number => Math.max(0, BYO_POSTURES.indexOf(p as ByoPosture));
/** Postures at least as strict as `floor` (an org or workspace can only tighten). */
export const byoOptionsFrom = (floor: string): ByoPosture[] => BYO_POSTURES.filter((p) => byoRank(p) >= byoRank(floor));

export interface ByoForm {
  /** "" = inherit the instance floor. */
  posture: ByoPosture | "";
  /** The instance floor: options below it are not offered. */
  floor: ByoPosture;
  /** The instance master switch is off: BYO is disabled everywhere and nothing here applies. */
  instanceOff: boolean;
  /** One CIDR per line / comma separated; empty = no org default list. */
  sourceCidrs: string;
  /** The instance default list (the org list must lie inside it when non-empty). */
  instanceCidrs: string[];
  requireContext: boolean;
  requireContextLocked: boolean;
  /** (phase 3, RFE-013) Org maximum agent-token age in days; "" = none (the instance value applies). */
  maxAgeDays: string;
  /** The instance maximum (`null` = none): the org value must not exceed it. */
  instanceMaxAgeDays: number | null;
}

export const AGENT_TOKEN_MAX_AGE_MIN = 1;
export const AGENT_TOKEN_MAX_AGE_MAX = 3650;

/** Parses the days field: `""` = none, a whole number in 1…3650, else `undefined` (invalid). */
export function parseMaxAgeDays(s: string): number | null | undefined {
  const t = s.trim();
  if (t === "") return null;
  if (!/^\d{1,4}$/.test(t)) return undefined;
  const n = Number(t);
  return n >= AGENT_TOKEN_MAX_AGE_MIN && n <= AGENT_TOKEN_MAX_AGE_MAX ? n : undefined;
}

/** Client-side hint only (the server is authoritative): why the days field is not acceptable. */
export function maxAgeDaysProblem(s: string, instance: number | null): string | undefined {
  const n = parseMaxAgeDays(s);
  if (n === undefined) return `Enter a whole number of days from ${AGENT_TOKEN_MAX_AGE_MIN} to ${AGENT_TOKEN_MAX_AGE_MAX}, or leave it empty.`;
  if (n !== null && instance !== null && n > instance) return `The instance limit is ${instance} days; an organization can only tighten it.`;
  return undefined;
}

export interface GovForm {
  llmPosture: "open" | "sanctioned_only";
  llmPostureLocked: boolean;
  endpoints: EndpointForm[];
  egressPosture: "open" | "credentialed_only";
  egressPostureLocked: boolean;
  /** (RFE-003) browser tunnels (open lane). */
  openLane: OpenLane;
  openLaneLocked: boolean;
  allowlist: { host: string; path: string }[];
  /** (RFE-025) BYO agents. */
  byo: ByoForm;
}

export const splitList = (s: string): string[] =>
  s.split(/[\s,]+/).map((x) => x.trim()).filter(Boolean);
const joinList = (l: string[] | undefined) => (l ?? []).join("\n");

/** Builds the editable form from the stored document, laid over the effective view (endpoint list + locks). */
export function formFromDocument(
  doc: Partial<GovDocument> | null | undefined,
  eff: EffectiveGovernance,
  instance?: InstanceGovernance | null,
): GovForm {
  const stored = doc?.llm?.endpoints ?? {};
  return {
    byo: byoFormFrom(doc?.byo, eff, instance, doc?.agent_tokens),
    llmPosture: (doc?.llm?.posture as GovForm["llmPosture"]) ?? (eff.llm.posture.value as GovForm["llmPosture"]),
    llmPostureLocked: eff.llm.posture.source === "instance",
    endpoints: eff.llm.endpoints.map((e) => {
      const d = stored[e.id];
      return {
        id: e.id,
        provider: e.provider,
        display_name: e.display_name,
        allowed: d ? d.allowed : false,
        key_scopes: d?.key_scopes ?? [],
        allowed_models: joinList(d?.allowed_models),
        org_ids: joinList(d?.origin?.allowed_organization_ids),
        workspace_ids: joinList(d?.origin?.allowed_workspace_ids),
        team_ids: joinList(d?.origin?.allowed_team_ids),
        locked: {
          permitted: e.permitted.source === "instance",
          key_scopes: e.key_scopes.source === "instance",
          allowed_models: e.allowed_models.source === "instance",
        },
      };
    }),
    egressPosture: (doc?.egress?.posture as GovForm["egressPosture"]) ?? (eff.egress.posture.value as GovForm["egressPosture"]),
    egressPostureLocked: eff.egress.posture.source === "instance",
    openLane: (doc?.egress?.open_lane as OpenLane | undefined) ?? (eff.egress.open_lane.value as OpenLane),
    openLaneLocked: eff.egress.open_lane.source === "instance",
    allowlist: (doc?.egress?.allowlist ?? eff.egress.org_allowlist).map((e) => ({ host: e.host, path: e.path ?? "" })),
  };
}

function byoFormFrom(
  stored: GovDocument["byo"] | undefined,
  eff: EffectiveGovernance,
  instance?: InstanceGovernance | null,
  tokens?: GovDocument["agent_tokens"] | null,
): ByoForm {
  const e = eff.byo;
  // The floor: the instance's own value when known, else the effective value when the instance
  // decided it (an org-decided value is stricter than the floor and says nothing about it).
  const floor = (instance?.byo?.minimum_posture ?? (e.posture.source === "instance" ? e.posture.value : "enabled")) as ByoPosture;
  return {
    posture: (stored?.posture as ByoPosture | undefined) ?? "",
    floor,
    instanceOff: instance?.byo ? !instance.byo.enabled : false,
    sourceCidrs: (stored?.source_cidrs ?? []).join("\n"),
    instanceCidrs: instance?.byo?.default_source_cidrs ?? [],
    requireContext: stored?.require_context_credentials ?? false,
    requireContextLocked: e.require_context_credentials.source === "instance" && e.require_context_credentials.value,
    maxAgeDays: tokens?.max_age_days != null ? String(tokens.max_age_days) : "",
    // Known from the instance view; else from the effective view when the instance decided it.
    instanceMaxAgeDays:
      instance?.agent_tokens?.max_age_days ??
      (eff.agent_tokens?.max_age_days.source === "instance" ? eff.agent_tokens.max_age_days.value : null),
  };
}

/** Read-only form for members: shows effective values (locked flags kept for labelling). */
export function formFromEffective(eff: EffectiveGovernance): GovForm {
  const base = formFromDocument(null, eff);
  return {
    ...base,
    byo: {
      ...base.byo,
      posture: eff.byo.posture.value as ByoPosture,
      sourceCidrs: (eff.byo.source_cidrs.value ?? []).join("\n"),
      requireContext: eff.byo.require_context_credentials.value,
      maxAgeDays: eff.agent_tokens.max_age_days.value != null ? String(eff.agent_tokens.max_age_days.value) : "",
    },
    llmPosture: eff.llm.posture.value as GovForm["llmPosture"],
    egressPosture: eff.egress.posture.value as GovForm["egressPosture"],
    openLane: eff.egress.open_lane.value as OpenLane,
    endpoints: base.endpoints.map((f, i) => {
      const e = eff.llm.endpoints[i];
      return {
        ...f,
        allowed: e.permitted.value,
        key_scopes: e.key_scopes.value,
        allowed_models: joinList(e.allowed_models.value),
        org_ids: joinList(e.origin_rule?.allowed_organization_ids),
        workspace_ids: joinList(e.origin_rule?.allowed_workspace_ids),
        team_ids: joinList(e.origin_rule?.allowed_team_ids),
      };
    }),
  };
}

export function formToDocument(f: GovForm): GovDocument {
  const endpoints: Record<string, GovEndpointDoc> = {};
  for (const e of f.endpoints) {
    endpoints[e.id] = {
      allowed: e.allowed,
      key_scopes: e.key_scopes,
      allowed_models: splitList(e.allowed_models),
      origin: {
        allowed_organization_ids: splitList(e.org_ids),
        allowed_workspace_ids: splitList(e.workspace_ids),
        allowed_team_ids: splitList(e.team_ids),
      },
    };
  }
  const cidrs = splitList(f.byo.sourceCidrs);
  // The server normalizes an all-default `byo` away, so sending it always is harmless.
  const byo: NonNullable<GovDocument["byo"]> = {
    posture: f.byo.posture || null,
    source_cidrs: cidrs,
    require_context_credentials: f.byo.requireContext,
  };
  // Empty = no org limit. An unparsable field never reaches here: the view disables saving
  // (`maxAgeDaysProblem`), and the server validates it again (422 `governance_invalid`).
  const days = parseMaxAgeDays(f.byo.maxAgeDays) ?? null;
  return {
    byo,
    agent_tokens: { max_age_days: days },
    llm: { posture: f.llmPosture, endpoints },
    egress: {
      posture: f.egressPosture,
      open_lane: f.openLane,
      allowlist: f.allowlist
        .filter((e) => e.host.trim())
        .map((e) => (e.path.trim() ? { host: e.host.trim(), path: e.path.trim() } : { host: e.host.trim() })),
    },
  };
}

export function isStale(err: unknown): boolean {
  return err instanceof ApiError && err.code === "stale_revision";
}

export interface GovIssues {
  /** Normalized JSON path (`/llm/endpoints/x/allowed_models/0`) -> messages. */
  byPath: Record<string, string[]>;
  general: string[];
}

export function normalizePath(p: string): string {
  let s = p.replace(/^\$\.?/, "");
  s = s.includes("/") ? s.replace(/\[(\d+)\]/g, "/$1") : s.replace(/\[(\d+)\]/g, ".$1").replace(/\./g, "/");
  return s.startsWith("/") ? s : `/${s}`;
}

/** Maps `governance_invalid` / `governance_below_instance_minimum` `details.fields[]` by JSON path. */
export function governanceIssues(err: unknown): GovIssues {
  const out: GovIssues = { byPath: {}, general: [] };
  if (!(err instanceof ApiError) || (err.code !== "governance_invalid" && err.code !== "governance_below_instance_minimum"))
    return out;
  const d = err.details as { fields?: { path?: unknown; message?: unknown }[] } | null;
  const fields = Array.isArray(d?.fields) ? d.fields : [];
  if (fields.length === 0) out.general.push(err.message);
  for (const f of fields) {
    const message = typeof f.message === "string" ? f.message : "Invalid value";
    if (typeof f.path === "string" && f.path) (out.byPath[normalizePath(f.path)] ??= []).push(message);
    else out.general.push(message);
  }
  return out;
}

/** Messages for `prefix` and everything beneath it. */
export function issuesUnder(issues: GovIssues, prefix: string): string[] {
  return Object.entries(issues.byPath)
    .filter(([p]) => p === prefix || p.startsWith(`${prefix}/`))
    .flatMap(([p, m]) => (p === prefix ? m : m.map((x) => `${p.slice(prefix.length + 1)}: ${x}`)));
}
