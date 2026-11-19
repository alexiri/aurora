"use client";

import { StatusLine, type StatusTone } from "@/components/ui/status-line";
import { useAgentPendingActionApprovals, useAgentPendingApprovals } from "@/lib/api/approvals";
import { useConversations } from "@/lib/api/conversations";
import { useModelReadiness } from "@/lib/api/credential-grants";

export type AgentStateWord = "Ready" | "Working" | "Needs you" | "Can't answer";

/** The state, most urgent first: the model is unusable, something waits for a decision, a turn runs, otherwise ready. */
export function agentStateOf(s: { cantAnswer: boolean; needs: number; working: boolean }): { word: AgentStateWord; tone: StatusTone } {
  if (s.cantAnswer) return { word: "Can't answer", tone: "fail" };
  if (s.needs > 0) return { word: "Needs you", tone: "attention" };
  if (s.working) return { word: "Working", tone: "ok" };
  return { word: "Ready", tone: "ok" };
}

/**
 * (P5-POLISH-6, D-382) The one state in the agent's header: **Ready**, **Working**, **Needs you** or **Can't answer** (the
 * model is not usable: no usable AI key). The computer's start and stop are not header states (they are a quiet line in the
 * conversation and the Computer page). `canOwn` lets the model check run (its route is owners'); approvers see Needs you.
 */
export function AgentState({
  agentId,
  wsId,
  canOwn,
  canApprove,
  hosted,
}: {
  agentId: string;
  wsId: string;
  canOwn: boolean;
  canApprove: boolean;
  hosted: boolean;
}) {
  const readiness = useModelReadiness(agentId, canOwn && hosted);
  const gateway = useAgentPendingApprovals(wsId, agentId, canApprove);
  const actions = useAgentPendingActionApprovals(wsId, agentId, canApprove);
  const conversations = useConversations(agentId);
  const keys = readiness.data?.keys;
  const cantAnswer = Array.isArray(keys) && !keys.some((k) => k.usable);
  const needs = (gateway.data?.items?.length ?? 0) + (actions.data?.items?.length ?? 0);
  const working = (conversations.data?.items ?? []).some((c) => !!c.active_turn);
  const { word, tone } = agentStateOf({ cantAnswer, needs, working });
  return (
    <StatusLine tone={tone} className="whitespace-nowrap leading-4" data-testid="agent-state">
      {word}
    </StatusLine>
  );
}
