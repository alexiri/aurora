"use client";

import { useEffect, useState } from "react";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { ApiError } from "@/lib/api/client";
import { useCanApprove, useDecide, usePendingApprovals } from "@/lib/api/approvals";
import type { TranscriptEvent } from "@/lib/api/conversations";
import { approvalStatus, type TranscriptState } from "@/lib/chat/transcript";

const s = (v: unknown): string => (typeof v === "string" ? v : "");

/** `m:ss` until `deadline`, or `null` when there is none. Never negative. */
export function countdown(deadline: string | undefined, now: number): string | null {
  if (!deadline) return null;
  const t = Date.parse(deadline);
  if (Number.isNaN(t)) return null;
  const left = Math.max(0, Math.floor((t - now) / 1000));
  return `${Math.floor(left / 60)}:${String(left % 60).padStart(2, "0")}`;
}

function useNow(active: boolean): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!active) return;
    const id = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(id);
  }, [active]);
  return now;
}

/**
 * The inline approval card in the agent column (compact variant of the Approvals page card, D-102): a HELD badge,
 * the countdown, the request line and rule name (from the workspace's pending approvals), Approve and Deny for
 * `can_approve` users, "Waiting for an approver" for everyone else, and one line with the decision once decided.
 */
export function ApprovalCard({ ev, state, wsId }: { ev: TranscriptEvent; state: TranscriptState; wsId: string }) {
  const id = s(ev.payload.approval_id);
  const st = approvalStatus(state, id);
  const settled = st.status !== "pending";
  const decide = useDecide(wsId);
  const canApprove = useCanApprove(wsId);
  const pending = usePendingApprovals(wsId, !settled);
  const detail = Array.isArray(pending.data?.items) ? pending.data.items.find((a) => a.id === id) : undefined;
  const now = useNow(!settled && !!detail);
  const left = settled ? null : countdown(detail?.deadline, now);

  if (settled) {
    const label = st.status === "expired" ? "Expired" : st.status === "approve" ? "Approved" : "Denied";
    return (
      <div role="status" className="flex max-w-[min(88%,720px)] flex-wrap items-center gap-2 rounded-card rounded-tl-sm border border-pending-line bg-pending-panel px-3 py-2 text-sm text-ink">
        <Badge tone={st.status === "approve" ? "accent" : st.status === "deny" ? "danger" : "neutral"}>
          {label}
          {st.by ? ` by ${st.by}` : ""}
        </Badge>
        <span className="min-w-0 break-words text-ink-2">{s(ev.payload.summary)}</span>
      </div>
    );
  }
  return (
    <div className="max-w-[min(88%,720px)] space-y-2 rounded-card rounded-tl-sm border border-pending-line bg-pending-panel p-3 text-sm text-ink">
      <div className="flex flex-wrap items-center gap-2">
        <Badge tone="pending">Held</Badge>
        <p className="font-semibold text-pending-ink">Approval requested</p>
        {left ? <span className="ml-auto text-xs tabular-nums text-ink-2">Expires in {left}</span> : null}
      </div>
      <p className="break-words">{s(ev.payload.summary)}</p>
      {detail ? (
        <p className="flex min-w-0 flex-wrap items-center gap-2 text-xs text-ink-2">
          <Badge>{detail.method}</Badge>
          <span className="min-w-0 break-all font-mono">
            {detail.host}
            {detail.path}
          </span>
          {detail.rule_name ? <span>Rule: {detail.rule_name}</span> : null}
        </p>
      ) : null}
      {!canApprove ? (
        <p role="status" className="text-ink-2">
          Waiting for an approver
        </p>
      ) : (
        <div className="grid grid-cols-2 gap-2 sm:flex">
          <Button className="min-h-11 sm:min-h-10" variant="approve" disabled={decide.isPending} onClick={() => decide.mutate({ id, decision: "approve" })}>
            Approve
          </Button>
          <Button className="min-h-11 sm:min-h-10" variant="outline" disabled={decide.isPending} onClick={() => decide.mutate({ id, decision: "deny" })}>
            Deny
          </Button>
        </div>
      )}
      {decide.error ? (
        <p role="alert" className="text-danger-ink">
          {decide.error instanceof ApiError && decide.error.status === 409 ? "Already decided." : "Could not record the decision."}
        </p>
      ) : null}
    </div>
  );
}

/** An action approval (decided in the Approvals page): the same pending panel, one line. */
export function ActionApprovalNote({ summary, wsId }: { summary: string; wsId: string }) {
  const canApprove = useCanApprove(wsId);
  return (
    <div className="flex max-w-[min(88%,720px)] flex-wrap items-center gap-2 rounded-card rounded-tl-sm border border-pending-line bg-pending-panel px-3 py-2 text-sm text-pending-ink">
      <Badge tone="pending">Held</Badge>
      <span className="min-w-0 break-words">
        Action approval: {summary} {canApprove ? "(decide in Approvals)" : "(waiting for an approver)"}
      </span>
    </div>
  );
}
