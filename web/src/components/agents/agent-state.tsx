"use client";

import type { ReactNode } from "react";

import { StatusLine, type StatusTone } from "@/components/ui/status-line";
import { useAgentPendingActionApprovals, useAgentPendingApprovals } from "@/lib/api/approvals";
import { useAnswerReadiness } from "@/lib/api/answer-readiness";
import { useConversations, useSandbox } from "@/lib/api/conversations";
import type { components } from "@/lib/api/schema";
import { useStoppedTurns } from "@/lib/chat/stopped";

export type AgentStateWord = "Ready" | "Working" | "Needs you" | "Can't answer" | "Starting its computer…" | "Asleep";

type AnswerReason = components["schemas"]["AnswerReason"];

/** (C-1062) Why the agent cannot answer, in the header's words. */
export const CANT_ANSWER_REASON: Record<AnswerReason, string> = {
  no_ai_key: "no AI key",
  key_not_usable: "the key stopped working",
  no_model: "choose a model",
};

/** The computer's part of the state (C-1059 `SandboxView.computer`); absent while it has not loaded. */
export interface ComputerFacts {
  word: string;
  ready: boolean;
  working: boolean;
  asleep: boolean;
}

/**
 * (P5-REFINE-11, D-408) The state, most urgent first: Can't answer (no usable AI key, or its computer failed to start) →
 * Needs you → Working → Starting its computer… → Asleep → Ready. **Ready only when the computer answers** (`computer.ready`);
 * while the computer's data has not loaded (or is absent) the old words apply and the computer states are not shown.
 */
export function agentStateOf(s: {
  cantAnswer: boolean;
  needs: number;
  working: boolean;
  computer?: ComputerFacts | null;
}): { word: AgentStateWord; tone: StatusTone } {
  const c = s.computer ?? null;
  if (s.cantAnswer || c?.word === "failed") return { word: "Can't answer", tone: "fail" };
  if (s.needs > 0) return { word: "Needs you", tone: "attention" };
  if (s.working || c?.working) return { word: "Working", tone: "ok" };
  if (c && c.word === "starting") return { word: "Starting its computer…", tone: "idle" };
  if (c && (c.asleep || c.word === "stopping")) return { word: "Asleep", tone: "idle" };
  return { word: "Ready", tone: "ok" };
}

/**
 * (P5-REFINE-11, D-408) The one state in the agent's header: **Ready** (only when the computer answers), **Starting its
 * computer…** (a spinner, `aria-live` polite), **Asleep** (it wakes on the next message), **Working**, **Needs you** or
 * **Can't answer: <reason>**. A hosted agent only; a BYO agent shows no computer states. `action` is the slot of the primary
 * fix next to "Can't answer" (P5-REFINE-12 fills it).
 */
export function AgentState({
  agentId,
  wsId,
  canApprove,
  hosted,
  action,
}: {
  agentId: string;
  wsId: string;
  /** Kept for callers: the answer check is any member's now (C-1062), so it no longer depends on owning the workspace. */
  canOwn?: boolean;
  canApprove: boolean;
  hosted: boolean;
  action?: ReactNode;
}) {
  const readiness = useAnswerReadiness(agentId, hosted);
  const sandbox = useSandbox(agentId);
  const gateway = useAgentPendingApprovals(wsId, agentId, canApprove);
  const actions = useAgentPendingActionApprovals(wsId, agentId, canApprove);
  const conversations = useConversations(agentId);
  const answer = readiness.data;
  const cantAnswer = answer?.can_answer === false;
  const needs = (gateway.data?.items?.length ?? 0) + (actions.data?.items?.length ?? 0);
  // (chat-4, C-1075) A turn the person stopped (or that has `abort_requested`) is not "Working": the header is Ready at once,
  // and the row's `active_turn` follows with the next event.
  const stoppedTurns = useStoppedTurns();
  const running = (conversations.data?.items ?? []).filter((c) => !!c.active_turn && !c.active_turn.abort_requested && !stoppedTurns.has(c.active_turn.id));
  const stopping = (conversations.data?.items ?? []).some((c) => !!c.active_turn && (c.active_turn.abort_requested || stoppedTurns.has(c.active_turn.id)));
  const working = running.length > 0;
  const sandboxComputer = sandbox.data?.computer ?? null;
  // The computer's "working" includes the turn that is being stopped: do not let it keep the header on Working.
  const computer = sandboxComputer && stopping && !working ? { ...sandboxComputer, working: false } : sandboxComputer;
  const { word, tone } = agentStateOf({ cantAnswer, needs, working, computer });
  const starting = word === "Starting its computer…";
  const reason = cantAnswer
    ? answer?.reason
      ? CANT_ANSWER_REASON[answer.reason]
      : null
    : word === "Can't answer"
      ? "its computer couldn't start"
      : null;
  return (
    <StatusLine tone={tone} className="whitespace-nowrap leading-4" data-testid="agent-state" action={word === "Can't answer" ? action : undefined}>
      <span aria-live="polite">
        {starting ? <span aria-hidden="true" className="mr-1.5 inline-block size-3.5 animate-spin rounded-full border-2 border-line-strong border-t-accent align-middle motion-reduce:animate-none" /> : null}
        {word === "Can't answer" ? (reason ? `Can't answer: ${reason}` : "Can't answer") : word}
      </span>
    </StatusLine>
  );
}
