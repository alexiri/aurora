"use client";

// Parked approvals (phase 5, P4-011B-4, D-264, D-265(d), C-822, C-836, docs/14 §Parked approvals): the chip, the
// sentences and the post-decision states the Approvals page, the chat's inline approval and the overview share.
import { Badge } from "@/components/ui/badge";
import { useApprovalLimits } from "@/lib/api/policy";
import type { Approval } from "@/lib/api/approvals";

type Tone = "accent" | "neutral" | "pending" | "danger";

/** "Parked": the agent was refused at once and has stopped waiting. Held approvals show no chip. */
export function ParkedChip() {
  return <Badge tone="pending">Parked</Badge>;
}

/** Minutes of the resume window, rounded up, for the sentences below. */
export function windowMinutes(secs: number | undefined): number | undefined {
  return secs === undefined ? undefined : Math.max(1, Math.ceil(secs / 60));
}

/** "The agent has stopped waiting. If you approve, it will be told and can repeat this exact request once within N minutes." */
export function parkedSentence(minutes: number | undefined): string {
  const within = minutes === undefined ? "within the resume window" : `within ${minutes} ${minutes === 1 ? "minute" : "minutes"}`;
  return `The agent has stopped waiting. If you approve, it will be told and can repeat this exact request once ${within}.`;
}

/** The sentence with the live resume window (`GET /v1/policy/approval-limits`). */
export function ParkedNote({ className }: { className?: string }) {
  const limits = useApprovalLimits().data;
  return (
    <p className={className ?? "text-sm text-pending-ink"} data-testid="parked-note">
      {parkedSentence(windowMinutes(limits?.park.resume_window_secs))}
    </p>
  );
}

const time = (iso?: string | null) =>
  iso ? new Date(iso).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" }) : "";

/**
 * What became of a decided parked approval, from `status` and `resume_state` (C-822): approved and waiting for the
 * agent's repeat, used, not used, revoked; or abandoned by a revocation. `null` for a held or still-pending approval.
 */
export function resumeStatus(a: Pick<Approval, "parked" | "status" | "resume_state" | "resume_expires_at">): { text: string; tone: Tone } | null {
  if (!a.parked) return null;
  if (a.status === "abandoned") return { text: "No longer needed", tone: "neutral" };
  if (a.status !== "approved") return null;
  switch (a.resume_state) {
    case "ready":
      return { text: `Approved — waiting for the agent to retry (until ${time(a.resume_expires_at)})`, tone: "accent" };
    case "consumed":
      return { text: "Used", tone: "accent" };
    case "expired":
      return { text: "Not used (window ended)", tone: "neutral" };
    case "revoked":
      return { text: "Revoked", tone: "danger" };
    default:
      return null;
  }
}

/** The post-decision state of a parked approval as a badge; nothing for a held one. */
export function ResumeBadge({ approval }: { approval: Pick<Approval, "parked" | "status" | "resume_state" | "resume_expires_at"> }) {
  const s = resumeStatus(approval);
  return s ? (
    <Badge tone={s.tone} className="normal-case tracking-normal" data-testid="resume-state">
      {s.text}
    </Badge>
  ) : null;
}
