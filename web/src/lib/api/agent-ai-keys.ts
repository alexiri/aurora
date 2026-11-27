"use client";

import { useQuery } from "@tanstack/react-query";

import { api } from "./client";
import type { components } from "./schema";
import { unwrap } from "./unwrap";

type S = components["schemas"];

/** (C-1168) One AI key that exists in the agent's organization or workspace and is not granted to the agent yet. */
export type AgentAiKey = S["AgentAiKey"];

export const agentAiKeysKey = (agentId: string) => ["agent-ai-keys", agentId] as const;

/**
 * (FIX-249, C-1168) The AI keys this agent does not have yet that exist in its organization or
 * workspace: the starter key first, then the dispatcher's order, an unusable key listed with its
 * reason in plain words. Metadata only, owners of the agent's workspace only.
 */
export function useAgentAiKeys(agentId: string | undefined, enabled = true) {
  return useQuery({
    queryKey: agentAiKeysKey(agentId ?? ""),
    queryFn: () => unwrap(api.GET("/v1/agents/{agent}/ai-keys", { params: { path: { agent: agentId! } } })),
    enabled: enabled && !!agentId,
    retry: false,
  });
}
