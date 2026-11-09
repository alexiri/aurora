"use client";

import { useQuery } from "@tanstack/react-query";

import { api } from "./client";
import type { components } from "./schema";
import { unwrap } from "./unwrap";

type S = components["schemas"];
/** (phase 3, RFE-013, C-313) One live BYO agent token in the stale / expiring lists. */
export type AgentTokenEntry = S["AgentTokenEntry"];
/** (C-314) `Agent.byo.token`. */
export type TokenView = S["TokenView"];

export type TokenState = "expired" | "expiring" | "stale";

/** Tokens with these flags get a badge; a token past its expiry is `expired`, not `expiring`. */
export interface TokenFlags {
  expires_at?: string | null;
  stale: boolean;
  expiring: boolean;
}

/**
 * The badges a token carries, most urgent first: `expired` (the expiry has passed), `expiring`
 * (the server's 14-day window), `stale` (unused for 30 days). Meaning is also carried by the
 * badge text, never by colour alone.
 */
export function tokenStates(t: TokenFlags | null | undefined, now: number = Date.now()): TokenState[] {
  if (!t) return [];
  const out: TokenState[] = [];
  const expired = !!t.expires_at && new Date(t.expires_at).getTime() <= now;
  if (expired) out.push("expired");
  else if (t.expiring) out.push("expiring");
  if (t.stale) out.push("stale");
  return out;
}

const DAY = 86_400_000;

/** "Expires in 12 days" / "Expired 3 days ago" / "Never expires". */
export function expiryText(expiresAt: string | null | undefined, now: number = Date.now()): string {
  if (!expiresAt) return "Never expires";
  const ms = new Date(expiresAt).getTime() - now;
  const days = Math.round(Math.abs(ms) / DAY);
  if (ms <= 0) return days === 0 ? "Expired today" : `Expired ${days} ${days === 1 ? "day" : "days"} ago`;
  if (ms < DAY) return "Expires within a day";
  return `Expires in ${days} ${days === 1 ? "day" : "days"}`;
}

export function lastUsedText(lastUsedAt: string | null | undefined): string {
  return lastUsedAt ? `Last used ${new Date(lastUsedAt).toLocaleString()}` : "Never used";
}

export const agentTokenKeys = {
  org: (org: string, q: string) => ["orgs", org, "agent-tokens", q] as const,
};

/**
 * Tokens that need attention in an organization (org owner/admin): unused for 30 days, or
 * expiring within 14 days (expired included), merged by agent.
 */
export function useOrgAttentionTokens(orgId: string, enabled: boolean) {
  return useQuery({
    queryKey: agentTokenKeys.org(orgId, "attention"),
    enabled,
    retry: false,
    queryFn: async (): Promise<AgentTokenEntry[]> => {
      const path = { org: orgId };
      const [stale, expiring] = await Promise.all([
        unwrap(api.GET("/v1/orgs/{org}/agent-tokens", { params: { path, query: { stale: true, limit: 100 } } })),
        unwrap(
          api.GET("/v1/orgs/{org}/agent-tokens", { params: { path, query: { expiring_within_days: 14, limit: 100 } } }),
        ),
      ]);
      return mergeTokens([...expiring.items, ...stale.items]);
    },
  });
}

/** Deduplicates by agent id (first occurrence wins), most urgent first (expiry soonest). */
export function mergeTokens(items: AgentTokenEntry[]): AgentTokenEntry[] {
  const seen = new Map<string, AgentTokenEntry>();
  for (const i of items) if (!seen.has(i.agent.id)) seen.set(i.agent.id, i);
  return [...seen.values()].sort((a, b) => {
    const ea = a.expires_at ? new Date(a.expires_at).getTime() : Infinity;
    const eb = b.expires_at ? new Date(b.expires_at).getTime() : Infinity;
    return ea - eb || a.agent.handle.localeCompare(b.agent.handle);
  });
}
