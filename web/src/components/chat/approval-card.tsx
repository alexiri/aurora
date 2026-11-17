"use client";

import { useParams } from "next/navigation";
import { useEffect, useState } from "react";

import { AlwaysAllowedNote, ApprovalQuestion } from "@/components/approvals/approval-question";
import { ParkedChip, ParkedNote } from "@/components/approvals/parked";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { ApiError } from "@/lib/api/client";
import { useCanApprove, useDecide, usePendingApprovals } from "@/lib/api/approvals";
import type { TranscriptEvent } from "@/lib/api/conversations";
import { useWorkspace } from "@/lib/api/queries";
import { approvalStatus, type TranscriptState } from "@/lib/chat/transcript";
import { orList, whoCanFromContacts } from "@/lib/who-can";

const s = (v: unknown): string => (typeof v === "string" ? v : "");

/** The element id of an approval's inline card: the status rail's "Show in conversation" scrolls to and focuses it. */
export const approvalAnchorId = (approvalId: string) => `approval-${approvalId}`;

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
 * The inline approval card in the agent column (P5-UX-16, D-348): the same question as the Approvals page
 * (`ApprovalQuestion`, from the workspace's pending approvals) while it waits, and one line with the answer once the
 * transcript has `approval.decided` or `approval.expired` (Always allow says what it changed).
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
  const params = useParams<{ org?: string; ws?: string }>();
  // "Waiting for <names>" for people who may not answer (C-983): the workspace's owners, from its contacts.
  const contacts = useWorkspace(wsId).data?.contacts;
  const approvers = whoCanFromContacts("workspace_approver", contacts, "decide_this").people.map((p) => p.name);
  // (C-982) The answer the conversation was told: Always allow names the change and what it let through.
  const decidedAlways = (() => {
    for (const e of state.events) {
      if (e.type === "approval.decided" && s(e.payload.approval_id) === id && typeof e.payload.always === "object" && e.payload.always) {
        return e.payload.always as { tool?: { name?: string }; also_approved?: number };
      }
    }
    return null;
  })();

  const parked = st.parked === true || ev.payload.parked === true || detail?.parked === true;

  if (settled) {
    const label = st.status === "expired" ? "Expired" : st.status === "approve" ? "Approved" : "Denied";
    // (P4-011B-4, C-825) A parked approval's story goes on after the decision: waiting for the agent's repeat, or not used.
    const after =
      parked && st.status === "approve"
        ? st.windowEnded
          ? "Not used (window ended)"
          : `The agent will be told${st.resumeExpiresAt ? ` and can repeat the request until ${new Date(st.resumeExpiresAt).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" })}` : ""}.`
        : null;
    return (
      <div
        id={approvalAnchorId(id)}
        tabIndex={-1}
        role="status"
        className="flex max-w-chat-card flex-wrap items-center gap-2 rounded-card rounded-tl-sm border border-pending-line bg-pending-panel px-3 py-2 text-sm text-ink">
        <Badge tone={st.status === "approve" ? "accent" : st.status === "deny" ? "danger" : "neutral"}>
          {label}
          {st.by ? ` by ${st.by}` : ""}
        </Badge>
        {parked ? <ParkedChip /> : null}
        <span className="min-w-0 break-words text-ink-2">{s(ev.payload.summary)}</span>
        {decidedAlways ? (
          <AlwaysAllowedNote
            className="w-full"
            agentName={s(ev.payload.agent_name) || "The agent"}
            agentHandle={s(ev.payload.agent_handle) || undefined}
            action={s(ev.payload.action)}
            appName={s(ev.payload.app_name) || undefined}
            alsoApproved={typeof decidedAlways.also_approved === "number" ? decidedAlways.also_approved : 0}
            org={params?.org ?? ""}
            ws={params?.ws ?? ""}
          />
        ) : null}
        {after ? (
          <span className="w-full text-xs text-ink-2" data-testid="parked-after">
            {after}
          </span>
        ) : null}
      </div>
    );
  }
  return (
    <div
      id={approvalAnchorId(id)}
      tabIndex={-1}
      className="max-w-chat-card space-y-2 rounded-card rounded-tl-sm border border-pending-line bg-pending-panel p-3 text-sm text-ink outline-none focus-visible:ring-2 focus-visible:ring-accent">
      {detail ? (
        <ApprovalQuestion
          approval={detail}
          now={now}
          compact
          canApprove={canApprove}
          approvers={approvers}
          org={params?.org ?? ""}
          ws={params?.ws ?? ""}
          pending={decide.isPending}
          onDecide={(decision, always) => decide.mutate({ id, decision, always })}
          message={
            decide.error
              ? decide.error instanceof ApiError && decide.error.status === 409
                ? "Already decided."
                : "Could not record the decision."
              : undefined
          }
        />
      ) : (
        // The pending list has not loaded yet: the sentence the transcript event carries, without answers.
        <>
          <div className="flex flex-wrap items-center gap-2">
            {parked ? <ParkedChip /> : <Badge tone="pending">Waiting</Badge>}
            <p className="break-words">{s(ev.payload.summary)}</p>
          </div>
          {parked ? <ParkedNote className="text-xs text-ink-2" /> : null}
          {canApprove ? (
            <div className="grid grid-cols-2 gap-2 sm:flex">
              <Button className="min-h-11 sm:min-h-10" variant="approve" disabled={decide.isPending} onClick={() => decide.mutate({ id, decision: "approve" })}>
                Allow once
              </Button>
              <Button className="min-h-11 sm:min-h-10" variant="outline" disabled={decide.isPending} onClick={() => decide.mutate({ id, decision: "deny" })}>
                Block
              </Button>
            </div>
          ) : (
            <p role="status" className="text-sm text-ink-2">
              {approvers.length > 0 ? `Waiting for ${orList(approvers)}` : "Waiting for an approver"}
            </p>
          )}
        </>
      )}
    </div>
  );
}

/** An action approval (decided in the Approvals page): the same pending panel, one line. */
export function ActionApprovalNote({ summary, wsId }: { summary: string; wsId: string }) {
  const canApprove = useCanApprove(wsId);
  return (
    <div className="flex max-w-chat-card flex-wrap items-center gap-2 rounded-card rounded-tl-sm border border-pending-line bg-pending-panel px-3 py-2 text-sm text-pending-ink">
      <Badge tone="pending">Held</Badge>
      <span className="min-w-0 break-words">
        Action approval: {summary} {canApprove ? "(decide in Approvals)" : "(waiting for an approver)"}
      </span>
    </div>
  );
}
