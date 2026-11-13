"use client";

import { useQuery } from "@tanstack/react-query";
import { useMemo } from "react";

import { useModelReadiness } from "@/lib/api/credential-grants";
import { chatApi } from "@/lib/api/conversations";
import { useSecrets, type SecretScope } from "@/lib/api/secrets";

import { computeGuide, type Guide, type GuideKey } from "./guide";

/**
 * How long until the next look for an answer (ms), or `false` to stop polling. Each look costs one list request plus
 * up to three event reads, so a workspace whose agent never answers backs off — 10 s for the first minute, then 30 s,
 * then 60 s — and stops after about 22 minutes. Focusing the window still refetches (react-query's default), and a
 * background tab does not poll at all.
 */
export function repliedPollInterval(looks: number, replied: boolean | undefined): number | false {
  if (replied === true) return false;
  if (looks < 6) return 10_000;
  if (looks < 12) return 30_000;
  if (looks < 30) return 60_000;
  return false;
}

/**
 * Has any conversation of the agent an answer from it? A `text` event is the agent's own words; an `error`
 * event (no key, no model) is not an answer. Looks at the three most recently active conversations.
 */
export function useAgentReplied(agentId: string | undefined, enabled = true) {
  return useQuery({
    queryKey: ["onboarding", "replied", agentId ?? ""] as const,
    enabled: enabled && !!agentId,
    retry: false,
    // While there is no answer yet the guide keeps watching, so the step ticks when the reply lands.
    refetchInterval: (q) => repliedPollInterval(q.state.dataUpdateCount, q.state.data),
    queryFn: async (): Promise<boolean> => {
      const list = await chatApi.list(agentId!, false);
      const recent = [...list.items]
        .sort((a, b) => (b.last_activity_at ?? b.created_at).localeCompare(a.last_activity_at ?? a.created_at))
        .slice(0, 3);
      for (const c of recent) {
        if (c.last_seq < 1) continue;
        const page = await chatApi.events(c.id, 0);
        if (page.items.some((e) => e.type === "text")) return true;
      }
      return false;
    },
  });
}

function llmKeys(items: { id: string; name: string; kind: string; status: string }[] | undefined, scope: GuideKey["scope"]): GuideKey[] {
  return (items ?? []).filter((s) => s.kind === "llm" && s.status === "active").map((s) => ({ id: s.id, name: s.name, scope }));
}

export interface GuideState {
  guide: Guide | null;
  /** The queries the guide needs have not answered yet. */
  loading: boolean;
  keys: GuideKey[];
  readinessError: unknown;
}

/**
 * The guide of one workspace and its agent, from real state: model readiness (owners), the LLM keys that exist
 * (this workspace's and the organization's), and whether the agent has answered.
 */
export function useGuide({ wsId, orgId, agentId, isOwner }: { wsId: string; orgId: string; agentId: string | undefined; isOwner: boolean }): GuideState {
  const wsScope: SecretScope = { kind: "workspace", id: wsId };
  const orgScope: SecretScope = { kind: "organization", id: orgId };
  const readiness = useModelReadiness(agentId ?? "", isOwner && !!agentId);
  const wsSecrets = useSecrets(wsScope);
  // Listing organization credentials needs org admin rights; a workspace owner who is not one gets none (not an error here).
  const orgSecrets = useSecrets(orgScope);
  const replied = useAgentReplied(agentId);

  const keys = useMemo(
    () => [...llmKeys(wsSecrets.data?.items, "workspace"), ...llmKeys(orgSecrets.data?.items, "organization")],
    [wsSecrets.data, orgSecrets.data],
  );
  const loading = !agentId || replied.isPending || (isOwner && (readiness.isPending || wsSecrets.isPending));
  const guide = useMemo(
    () =>
      loading
        ? null
        : computeGuide({ isOwner, readiness: readiness.data, keys, replied: replied.data === true }),
    [loading, isOwner, readiness.data, keys, replied.data],
  );
  return { guide, loading, keys, readinessError: readiness.error };
}
