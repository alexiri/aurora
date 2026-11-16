"use client";

import { useQueryClient } from "@tanstack/react-query";
import { useEffect, useMemo, useRef } from "react";

import type { ActionApproval } from "@/lib/api/action-approvals";
import {
  approvalKeys,
  useAgentPendingActionApprovals,
  useAgentPendingApprovals,
  useAgentRecentActivity,
  type Approval,
  type RequestLogEntry,
} from "@/lib/api/approvals";
import { useSandbox, type Conversation, type SandboxView } from "@/lib/api/conversations";
import { conversationTitle } from "@/lib/chat/layout";

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

export interface DecisionBadge {
  label: "Allow" | "Held" | "Deny" | "Failed";
  tone: "accent" | "pending" | "danger";
}

/** The rail's badge for a request-log row: an `error_code` reads Failed; allow/approved Allow; an expired hold Held; the rest Deny. */
export function decisionBadge(row: Pick<RequestLogEntry, "decision" | "error_code">): DecisionBadge {
  if (row.error_code || row.decision === "error") return { label: "Failed", tone: "danger" };
  if (row.decision === "allow" || row.decision === "approved") return { label: "Allow", tone: "accent" };
  if (row.decision === "approval_expired") return { label: "Held", tone: "pending" };
  return { label: "Deny", tone: "danger" };
}

/** One of the rows was denied or failed (a `danger-ink` dot on the collapsed icon). */
export const hasProblem = (rows: readonly Pick<RequestLogEntry, "decision" | "error_code">[]): boolean =>
  rows.some((r) => decisionBadge(r).tone === "danger");

export interface SandboxLook {
  tone: "neutral" | "accent" | "pending" | "danger";
  label: string;
  /** The sanitised failure reason, as the header pill's tooltip (FIX-025). */
  title?: string;
}

/** The sandbox state's label, tone and failure tooltip, shared by the header pill, the rail and the sheet. */
export function sandboxLook(sb: Pick<SandboxView, "state" | "failure_message" | "failure_code">): SandboxLook {
  const s = sb.state;
  const tone = s === "running" ? "accent" : s === "failed" ? "danger" : s === "starting" || s === "stopping" ? "pending" : "neutral";
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
  recent: RequestLogEntry[];
  recentLoading: boolean;
  recentError: boolean;
  problem: boolean;
  sandbox: SandboxView | undefined;
}

/**
 * The status rail's data (D-201), from existing routes: pending gateway approvals (C-700) and action approvals of
 * this agent (approvers only), the conversations' `active_turn`, the request log by agent, the sandbox endpoint.
 * Refresh: approvals on stream events (the stream hook invalidates the agent-scoped list); action approvals every
 * 10 s; the request log when a turn ends and every 30 s while visible.
 */
export function useAgentStatus({
  wsId,
  agentId,
  canApprove,
  conversations,
  open,
}: {
  wsId: string;
  agentId: string;
  canApprove: boolean;
  conversations: readonly Conversation[];
  open: OpenTurn;
}): AgentStatus {
  const qc = useQueryClient();
  const gateway = useAgentPendingApprovals(wsId, agentId, canApprove);
  const actions = useAgentPendingActionApprovals(wsId, agentId, canApprove);
  const activity = useAgentRecentActivity(wsId, agentId, 5);
  const sandbox = useSandbox(agentId);

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
  const working = useMemo(() => conversations.filter((c) => !!c.active_turn), [conversations]);
  const recent = useMemo(() => activity.data?.items ?? [], [activity.data]);

  // A turn ended (the open conversation's, or one fewer working conversation): its tool calls are now in the log.
  const runningCount = Math.max(working.length, open.active ? 1 : 0);
  const prev = useRef(runningCount);
  useEffect(() => {
    if (runningCount < prev.current) void qc.invalidateQueries({ queryKey: approvalKeys.recentActivity(wsId, agentId) });
    prev.current = runningCount;
  }, [runningCount, qc, wsId, agentId]);

  return {
    canApprove,
    needs,
    needsLoading: canApprove && (gateway.isPending || actions.isPending),
    working,
    recent,
    recentLoading: activity.isPending,
    recentError: activity.isError,
    problem: hasProblem(recent),
    sandbox: sandbox.data,
  };
}
