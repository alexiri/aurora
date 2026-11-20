"use client";

import { useQuery } from "@tanstack/react-query";

import { api } from "./client";
import type { components } from "./schema";
import { unwrap } from "./unwrap";

type S = components["schemas"];
/** (C-1062) Whether an agent can answer, why not, and what fixes it for the viewer. */
export type AnswerReadiness = S["AnswerReadiness"];
export type AnswerFix = S["AnswerFix"];

export const answerReadinessKey = (agentId: string) => ["answer-readiness", agentId] as const;

/**
 * (C-1062, D-414, D-408) Visible to everyone who can use the agent, so a member sees the same reason as an owner.
 * `fix` is chosen for the viewer: `give_key`, `add_key` or `ask_admin` (roles only, never names).
 */
export function useAnswerReadiness(agentId: string | undefined, enabled = true) {
  return useQuery({
    queryKey: answerReadinessKey(agentId ?? ""),
    queryFn: () => unwrap(api.GET("/v1/agents/{agent}/answer-readiness", { params: { path: { agent: agentId! } } })),
    enabled: enabled && !!agentId,
    retry: false,
  });
}
