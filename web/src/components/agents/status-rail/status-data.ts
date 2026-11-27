"use client";

import { useMemo } from "react";

import type { ActionApproval } from "@/lib/api/action-approvals";
import { useAgentPendingActionApprovals, useAgentPendingApprovals, type Approval, type RequestLogEntry } from "@/lib/api/approvals";
import { useSandbox, type Conversation, type SandboxView } from "@/lib/api/conversations";
import { useAgentTimeline, useTimelineStream, type TimelineItem } from "@/lib/api/timeline";
import { conversationTitle, isWorking } from "@/lib/chat/layout";
import { useStoppedTurns } from "@/lib/chat/stopped";

/** One thing waiting for a decision: a gateway hold or an action approval of this agent. */
export interface NeedsItem {
  key: string;
  kind: "gateway" | "action";
  id: string;
  conversationId: string | null;
  /** The conversation's title when it is known. */
  title: string | null;
  summary: string;
  deadline: string;
  /** The mono request line: `METHOD host/path` for a gateway hold, the action key for an action approval. */
  requestLine: string;
}

/** Gateway holds and action approvals as one list, soonest expiry first. */
export function mergeNeeds(
  gateway: readonly Approval[],
  actions: readonly ActionApproval[],
  titleOf: (conversationId: string) => string | null,
): NeedsItem[] {
  const items: NeedsItem[] = [
    ...gateway.map((a): NeedsItem => ({
      key: `g:${a.id}`,
      kind: "gateway",
      id: a.id,
      conversationId: a.conversation_id ?? null,
      title: a.conversation_id ? titleOf(a.conversation_id) : null,
      summary: a.summary.text || a.summary.title,
      deadline: a.deadline,
      requestLine: `${a.method} ${a.host}${a.path}`,
    })),
    ...actions.map((a): NeedsItem => ({
      key: `a:${a.id}`,
      kind: "action",
      id: a.id,
      conversationId: a.conversation_id ?? null,
      title: a.conversation_id ? titleOf(a.conversation_id) : null,
      summary: a.summary,
      deadline: a.expires_at,
      requestLine: a.action_key,
    })),
  ];
  return items.sort((x, y) => Date.parse(x.deadline) - Date.parse(y.deadline));
}

/** The request log without the agent's own model calls (a request that names its `llm_model`): thinking is not an action. */
export function actionsOnly<T extends Pick<RequestLogEntry, "llm_model">>(rows: readonly T[]): T[] {
  return rows.filter((r) => !r.llm_model);
}

/** How many timeline rows "What <Agent> did" shows (D-415). */
export const DID_LIMIT = 5;

/** One of the rows failed or was blocked (a `danger-ink` dot on the collapsed icon). */
export const hasProblem = (rows: readonly Pick<TimelineItem, "state">[]): boolean =>
  rows.some((r) => r.state === "failed" || r.state === "blocked");

export interface SandboxLook {
  tone: "neutral" | "accent" | "danger";
  label: string;
  /** The sanitised failure reason, as the header pill's tooltip (FIX-025). */
  title?: string;
}

/** The sandbox state's label, tone and failure tooltip, shared by the header pill, the rail and the sheet. */
export function sandboxLook(sb: Pick<SandboxView, "state" | "failure_message" | "failure_code">): SandboxLook {
  const s = sb.state;
  const tone = s === "running" ? "accent" : s === "failed" ? "danger" : "neutral";
  const title =
    s === "failed" && (sb.failure_message || sb.failure_code)
      ? `${sb.failure_message ?? ""}${sb.failure_code ? ` (${sb.failure_code})` : ""}`.trim()
      : undefined;
  return { tone, label: s === "starting" ? "Sandbox starting" : `Sandbox ${s}`, title };
}

/** (P5-UX-1) The computer's state in one word, as the agent header says it: Off, Starting, On, Stopping, Failed. */
export function computerWord(state: string): "Off" | "Starting" | "On" | "Stopping" | "Failed" {
  if (state === "running") return "On";
  if (state === "starting") return "Starting";
  if (state === "stopping") return "Stopping";
  if (state === "failed") return "Failed";
  return "Off";
}

/** The open conversation's live state, reported by its thread (the stream is read once, in the thread). */
export interface OpenTurn {
  active: boolean;
  /** The agent's latest activity line (the running tool), when it sent one. */
  line: string | null;
}

export interface AgentStatus {
  canApprove: boolean;
  needs: NeedsItem[];
  needsLoading: boolean;
  /** Conversations with a turn in flight (from the list's `active_turn`). */
  working: Conversation[];
  /** The last five timeline rows (D-415), newest first; updated in place by the timeline stream. */
  did: TimelineItem[];
  didLoading: boolean;
  didError: boolean;
  problem: boolean;
  sandbox: SandboxView | undefined;
}

/**
 * The status rail's data (D-201), from existing routes: pending gateway approvals (C-700) and action approvals of
 * this agent (approvers only), the conversations' `active_turn`, the agent's timeline (D-415), the sandbox endpoint.
 * Refresh: approvals on stream events (the stream hook invalidates the agent-scoped list); action approvals every
 * 10 s; the timeline over its own stream (rows are upserted in place).
 */
export function useAgentStatus({
  wsId,
  agentId,
  canApprove,
  conversations,
  computer = true,
}: {
  wsId: string;
  agentId: string;
  canApprove: boolean;
  conversations: readonly Conversation[];
  /** Read the agent's computer; off for a viewer who may not (no rail is shown to them). */
  computer?: boolean;
}): AgentStatus {
  const gateway = useAgentPendingApprovals(wsId, agentId, canApprove);
  const actions = useAgentPendingActionApprovals(wsId, agentId, canApprove);
  // (P5-REFINE-11, D-415) One timeline feed and one stream per agent page; the rail's three surfaces read this.
  const timeline = useAgentTimeline(agentId, {}, DID_LIMIT);
  useTimelineStream(agentId);
  const sandbox = useSandbox(agentId, computer);

  const byId = useMemo(() => new Map(conversations.map((c) => [c.id, c])), [conversations]);
  const needs = useMemo(
    () =>
      canApprove
        ? mergeNeeds(gateway.data?.items ?? [], actions.data?.items ?? [], (id) => {
            const c = byId.get(id);
            return c ? conversationTitle(c) : null;
          })
        : [],
    [canApprove, gateway.data, actions.data, byId],
  );
  // A turn Stop was pressed on is not "working" any more, though the list has not refetched (chat-4).
  const stoppedTurns = useStoppedTurns();
  const working = useMemo(() => conversations.filter((c) => isWorking(c, stoppedTurns)), [conversations, stoppedTurns]);
  // The newest five rows of the first page: the stream puts new rows on top and turns a running row into its outcome in place.
  const did = useMemo(() => (timeline.data?.pages[0]?.items ?? []).slice(0, DID_LIMIT), [timeline.data]);

  return {
    canApprove,
    needs,
    needsLoading: canApprove && (gateway.isPending || actions.isPending),
    working,
    did,
    didLoading: timeline.isPending,
    didError: timeline.isError,
    problem: hasProblem(did),
    sandbox: sandbox.data,
  };
}
