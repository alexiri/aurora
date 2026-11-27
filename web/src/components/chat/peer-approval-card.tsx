"use client";

import { useState } from "react";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { ApiError } from "@/lib/api/client";
import type { TranscriptEvent } from "@/lib/api/conversations";
import { useCanApprove, useDecide } from "@/lib/api/approvals";
import { useAskApprovers, useCancelPeerTask } from "@/lib/api/peers";

const s = (v: unknown): string => (typeof v === "string" ? v : "");

/** The notice codes of a delegated task's approval at the origin (FIX-252): the wait and its outcome. */
export const PEER_WAITING_CODE = "peer_waiting_for_approval" as const;
export const PEER_DECIDED_CODE = "peer_approval_decided" as const;

/** How the wait ended, read from the transcript: a later notice about the same approval, or the task closing. */
export function peerApprovalOutcome(
  events: TranscriptEvent[],
  approvalId: string,
  peerTaskId: string,
  seq: number,
): "approved" | "denied" | "expired" | "cancelled" | null {
  for (const e of events) {
    if ((e.seq ?? 0) <= seq || e.type !== "notice") continue;
    const code = s(e.payload.code);
    if (approvalId && code === PEER_DECIDED_CODE && s(e.payload.approval_id) === approvalId) {
      const o = s(e.payload.outcome);
      return o === "denied" ? "denied" : o === "expired" ? "expired" : "approved";
    }
    if (peerTaskId && code === "peer_task_closed" && s(e.payload.peer_task_id) === peerTaskId) {
      return "cancelled";
    }
  }
  return null;
}

const OUTCOME_LABEL: Record<"approved" | "denied" | "expired" | "cancelled", string> = {
  approved: "Approved",
  denied: "Denied",
  expired: "Expired",
  cancelled: "Cancelled",
};

/** The settled card: the outcome badge and the one line the notice carries (never a request field). */
export function PeerApprovalOutcome({ ev }: { ev: TranscriptEvent }) {
  const p = ev.payload;
  const outcome = s(p.outcome) === "denied" ? "denied" : s(p.outcome) === "expired" ? "expired" : "approved";
  return (
    <div
      role="status"
      data-testid="peer-approval-outcome"
      className="flex max-w-chat-card flex-wrap items-center gap-2 rounded-card rounded-tl-sm border border-pending-line bg-pending-panel px-3 py-2 text-sm text-ink">
      <Badge tone={outcome === "approved" ? "accent" : outcome === "denied" ? "danger" : "neutral"}>{OUTCOME_LABEL[outcome]}</Badge>
      <span className="min-w-0 break-words text-ink-2">{s(p.text)}</span>
    </div>
  );
}

/**
 * (FIX-252) A delegated agent's approval wait, as a card in the conversation the person asked in: who is waiting and
 * for what, in words. **Approve** and **Deny** only when the caller may decide in the approval's workspace (the API
 * says so through the workspace hook — never inferred here); otherwise **Ask an approver** and **Cancel the task**.
 * Deciding still needs `can_approve` at that workspace: being the requester adds nothing.
 */
export function PeerApprovalCard({ ev, events }: { ev: TranscriptEvent; events: TranscriptEvent[] }) {
  const p = ev.payload;
  const approvalId = s(p.approval_id);
  const peerTaskId = s(p.peer_task_id);
  // The approval's workspace: the API says whether this reader may decide there.
  const targetWs = s(p.workspace_id);
  const canDecide = useCanApprove(targetWs || undefined);
  const decide = useDecide(targetWs);
  const ask = useAskApprovers();
  const cancel = useCancelPeerTask();
  const [asked, setAsked] = useState(false);

  const outcome = peerApprovalOutcome(events, approvalId, peerTaskId, ev.seq ?? 0);
  // Settled: the card folds away; the conversation carries the outcome notice (or the close line).
  if (outcome) return null;

  const err = (e: unknown, fallback: string) =>
    e instanceof ApiError && e.message ? e.message : fallback;

  return (
    <div
      data-approval-card=""
      data-testid="peer-approval-card"
      className="max-w-chat-card space-y-2 rounded-card rounded-tl-sm border border-pending-line bg-pending-panel p-3 text-sm text-ink">
      <div className="flex flex-wrap items-center gap-2">
        <Badge>Waiting</Badge>
        <p className="min-w-0 break-words">{s(p.text)}</p>
      </div>
      {canDecide ? (
        <div className="grid grid-cols-2 gap-2 sm:flex">
          <Button
            data-testid="peer-approve"
            className="min-h-11 sm:min-h-10"
            variant="approve"
            disabled={decide.isPending}
            onClick={() => decide.mutate({ id: approvalId, decision: "approve" })}>
            Approve
          </Button>
          <Button
            data-testid="peer-deny"
            className="min-h-11 sm:min-h-10"
            variant="outline"
            disabled={decide.isPending}
            onClick={() => decide.mutate({ id: approvalId, decision: "deny" })}>
            Deny
          </Button>
        </div>
      ) : (
        <div className="flex flex-wrap items-center gap-2">
          {!asked ? (
            <Button
              data-testid="peer-ask"
              className="min-h-11 sm:min-h-10"
              variant="outline"
              disabled={ask.isPending || !approvalId}
              onClick={() =>
                ask.mutate(approvalId, {
                  onSuccess: () => setAsked(true),
                })
              }>
              Ask an approver
            </Button>
          ) : (
            <span role="status" data-testid="peer-asked" className="text-ink-2">
              Asked an approver.
            </span>
          )}
          <Button
            data-testid="peer-cancel"
            className="min-h-11 sm:min-h-10"
            variant="ghost"
            disabled={cancel.isPending || !peerTaskId}
            onClick={() => cancel.mutate(peerTaskId)}>
            Cancel the task
          </Button>
        </div>
      )}
      {ask.isError ? (
        <span role="alert" className="text-xs text-danger-ink">
          {err(ask.error, "Could not ask an approver.")}
        </span>
      ) : null}
      {cancel.isError ? (
        <span role="alert" className="text-xs text-danger-ink">
          {err(cancel.error, "Could not cancel the task.")}
        </span>
      ) : null}
      {decide.isError ? (
        <span role="alert" className="text-xs text-danger-ink">
          {decide.error instanceof ApiError && decide.error.status === 409
            ? "Already decided."
            : "Could not record the decision."}
        </span>
      ) : null}
    </div>
  );
}
