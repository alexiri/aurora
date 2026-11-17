"use client";

// (P5-UX-16, D-348, D-353, C-982, C-983, docs/14 §Agents, accounts and access §5) The one card an approval is shown in,
// on the Approvals page and in the conversation where the agent paused: the sentence ("Daneel wants to comment on an
// issue in Linear"), the app's details, the account, when it expires and three answers: Allow once, Always allow (only
// where the server says `can_always_allow`) and Block. People who may not answer read who will.
import Link from "next/link";
import { useEffect, useRef, useState } from "react";

import { ParkedChip, ParkedNote } from "@/components/approvals/parked";
import { Avatar } from "@/components/agent-avatar";
import { Button } from "@/components/ui/button";
import { ConfirmDialog } from "@/components/ui/confirm-dialog";
import { countdown, type Approval, type Decision } from "@/lib/api/approvals";
import { wsRoutes, orgRoutes } from "@/lib/routes";
import { cn } from "@/lib/utils";
import { orList } from "@/lib/who-can";

/** "Daneel wants to comment on an issue in Linear": strong agent, action and app (React text nodes only). */
export function Sentence({ a }: { a: Approval }) {
  const { agent, action, app } = a.sentence;
  return (
    <>
      <strong className="font-semibold">{agent.name}</strong> wants to <strong className="font-semibold">{action}</strong>
      {app ? (
        <>
          {" "}
          in <strong className="font-semibold">{app.name}</strong>
        </>
      ) : null}
    </>
  );
}

/** Where a rule-asked question's rule is edited: the workspace's or the organization's Internet access. */
function ruleHref(a: Approval, org: string, ws: string): string {
  return a.asked_by?.kind === "rule" && a.asked_by.scope === "organization" ? orgRoutes(org).internetAccess() : wsRoutes(org, ws).internetAccess();
}

export interface ApprovalQuestionProps {
  approval: Approval;
  now: number;
  /** Allow once / Block, or Always allow (`always`). */
  onDecide: (d: Decision, always?: boolean) => void;
  pending: boolean;
  message?: string;
  /** `?focus=<id>` (a notification's link): scrolled into view and outlined. */
  focused?: boolean;
  /** Whether the viewer may answer (`can_approve`); others read who will. */
  canApprove: boolean;
  /** The people who may answer, for "Waiting for …". */
  approvers?: string[];
  org: string;
  ws: string;
  /** The conversation's compact variant. */
  compact?: boolean;
}

export function ApprovalQuestion({ approval: a, now, onDecide, pending, message, focused = false, canApprove, approvers = [], org, ws, compact = false }: ApprovalQuestionProps) {
  const [confirming, setConfirming] = useState(false);
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (focused) ref.current?.scrollIntoView?.({ block: "center" });
  }, [focused]);
  const left = countdown(a.deadline, now);
  return (
    <div ref={ref} id={`approval-${a.id}`} tabIndex={focused ? -1 : undefined} aria-current={focused ? "true" : undefined} className="space-y-3 text-ink outline-none">
      <div className="flex flex-wrap items-center gap-x-2 gap-y-1 text-sm">
        <Avatar name={a.sentence.agent.name} tintKey={a.sentence.agent.id} size={24} />
        {a.agent ? <span className="font-mono text-muted">@{a.agent.handle}</span> : null}
        {a.parked ? <ParkedChip /> : null}
        {left ? <span className="ml-auto font-medium tabular-nums text-pending-ink">Expires in {left}</span> : null}
      </div>
      <p className={cn("font-display font-medium", compact ? "text-base" : "text-xl")}>
        <Sentence a={a} />
      </p>
      {a.summary.text ? <p className="whitespace-pre-wrap text-sm text-ink-2">{a.summary.text}</p> : null}
      <dl className="grid grid-cols-[max-content_1fr] gap-x-4 gap-y-1 text-sm">
        {a.summary.fields.map((f, i) => (
          <div key={i} className="contents">
            <dt className="text-muted">{f.label}</dt>
            <dd className="break-words">{f.value}</dd>
          </div>
        ))}
        {a.sentence.account ? (
          <div className="contents">
            <dt className="text-muted">Account</dt>
            <dd className="break-words">{a.sentence.account.name}</dd>
          </div>
        ) : null}
      </dl>
      <p className="flex min-w-0 flex-wrap items-center gap-2 rounded-control bg-canvas px-3 py-2 font-mono text-xs text-ink-2">
        <span className="rounded bg-subtle px-1.5 py-0.5 font-semibold text-ink">{a.method}</span>
        <span className="min-w-0 break-all">
          {a.host}
          {a.path}
        </span>
      </p>
      {a.parked ? <ParkedNote /> : null}
      {a.asked_by?.kind === "rule" ? (
        <p className="text-sm text-ink-2" data-testid="asked-by-rule">
          Asked by the rule{" "}
          <Link href={ruleHref(a, org, ws)} className="font-medium text-link underline">
            {a.asked_by.rule_name}
          </Link>
          .
        </p>
      ) : null}
      {a.context_label ? (
        <p className="text-xs text-muted" data-testid="context-label">
          Run: {a.context_label}
        </p>
      ) : null}
      {message ? (
        <p role="alert" className="rounded-control border border-pending-line bg-pending-panel p-3 text-sm text-pending-ink">
          {message}
        </p>
      ) : null}
      {canApprove ? (
        // At 390 px: Allow once and Block side by side, Always allow under them (44 px targets).
        <div className="grid grid-cols-2 items-center gap-2 sm:flex sm:flex-wrap">
          <Button className="min-h-11 sm:min-h-10" size={compact ? "default" : "lg"} variant="approve" disabled={pending} onClick={() => (a.parked ? setConfirming(true) : onDecide("approve"))}>
            Allow once
          </Button>
          {a.can_always_allow ? (
            <Button className="col-span-2 order-last min-h-11 sm:order-none sm:col-auto sm:min-h-10" size={compact ? "default" : "lg"} variant="outline" disabled={pending} onClick={() => onDecide("approve", true)}>
              Always allow
            </Button>
          ) : null}
          <Button className="min-h-11 sm:min-h-10" size={compact ? "default" : "lg"} variant="outline" disabled={pending} onClick={() => onDecide("deny")}>
            Block
          </Button>
          {a.asked_by?.kind === "rule" ? <span className="col-span-2 order-last text-xs text-muted sm:order-none sm:col-auto">Always allow is not offered for a rule&apos;s question.</span> : null}
        </div>
      ) : (
        <p role="status" className="text-sm text-ink-2">
          {approvers.length > 0 ? `Waiting for ${orList(approvers)}` : "Waiting for an approver"}
        </p>
      )}
      {a.parked ? (
        <ConfirmDialog
          open={confirming}
          onOpenChange={setConfirming}
          title="Allow this request once?"
          confirmLabel="Allow once"
          busy={pending}
          onConfirm={() => {
            setConfirming(false);
            onDecide("approve");
          }}
        >
          <ParkedNote className="text-sm text-ink-2" />
        </ConfirmDialog>
      ) : null}
    </div>
  );
}

/**
 * What Always allow did, in the words of the board: "Daneel will no longer ask before creating issues in Linear.
 * Change this on Daneel's Access tab." with the number of other requests it let through.
 */
export function AlwaysAllowedNote({ agentName, agentHandle, action, appName, alsoApproved, org, ws, className }: { agentName: string; agentHandle?: string; action: string; appName?: string; alsoApproved: number; org: string; ws: string; className?: string }) {
  const accessHref = agentHandle ? wsRoutes(org, ws).agent(agentHandle, { tab: "access" }) : wsRoutes(org, ws).accounts();
  return (
    <p className={cn("text-sm text-ink-2", className)} data-testid="always-allowed">
      {agentName} will no longer ask before it wants to {action}
      {appName ? ` in ${appName}` : ""}.
      {alsoApproved > 0 ? ` ${alsoApproved} other waiting ${alsoApproved === 1 ? "request" : "requests"} of the same kind went ahead too.` : ""} Change this on{" "}
      <Link href={accessHref} className="font-medium text-link underline">
        {agentName}&apos;s Access tab
      </Link>
      .
    </p>
  );
}
