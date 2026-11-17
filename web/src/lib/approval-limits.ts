// The live approval-timeout bounds (C-829, `GET /v1/policy/approval-limits`) as the rule editor and the grant dialog
// show them (docs/14 §Parked approvals). The API stays authoritative; these are hints.
import type { components } from "@/lib/api/schema";

export type ApprovalLimits = components["schemas"]["ApprovalLimits"];

/** `90` → "90 s", `300` → "5 min", `3600` → "1 h", `86400` → "24 h". */
export function fmtSecs(secs: number): string {
  if (secs < 120 && secs % 60 !== 0) return `${secs} s`;
  if (secs % 3600 === 0) return `${secs / 3600} h`;
  if (secs % 60 === 0) return `${secs / 60} min`;
  return `${Math.round(secs / 60)} min`;
}

/** The highest timeout a rule or grant may ask for: the park maximum while parking is on, else the hold maximum. */
export function effectiveMax(l: ApprovalLimits | undefined): number | undefined {
  return l ? (l.park.enabled ? l.park.max_timeout_secs : l.max_timeout_secs) : undefined;
}

/** The hint under an *Approval timeout* field. */
export function limitsHint(l: ApprovalLimits | undefined, min = 60): string {
  if (!l) return `Empty = the instance default. At least ${min} s, at most the instance maximum.`;
  const max = effectiveMax(l) as number;
  const base = `Empty = the instance default (${fmtSecs(l.timeout_secs)}). At least ${min} s, at most ${fmtSecs(max)}.`;
  return l.park.enabled
    ? `${base} Above ${fmtSecs(l.park.threshold_secs)} the agent stops waiting and is told when you decide (up to ${fmtSecs(max)}).`
    : base;
}
