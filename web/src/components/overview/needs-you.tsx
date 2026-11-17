"use client";

import Link from "next/link";

import { AccountAttentionItem, useWorkspaceAttention } from "@/components/accounts/attention";
import { ParkedChip } from "@/components/approvals/parked";
import { Badge } from "@/components/ui/badge";
import { buttonVariants } from "@/components/ui/button";
import { countdown, useCanApprove, useNow, usePendingApprovals } from "@/lib/api/approvals";
import { cn } from "@/lib/utils";

/**
 * The Overview's **Needs you** card (the Overview board, C-945): what waits for a person, in one place: the approvals an
 * agent asked for (for people who may decide) and the accounts that stopped working (with their fix, or who can fix
 * them). Nothing when nothing waits.
 */
export function NeedsYou({ wsId, base }: { wsId: string; base: string }) {
  const canApprove = useCanApprove(wsId);
  const pending = usePendingApprovals(wsId, canApprove);
  const broken = useWorkspaceAttention(wsId);
  const clock = useNow(1000);
  const approvals = canApprove ? (pending.data?.items ?? []) : [];
  const urgent = approvals.slice(0, 2);
  const total = approvals.length + broken.length;
  if (total === 0) return null;

  return (
    <section aria-labelledby="needs-you-h" className="rounded-card border border-pending-line bg-pending-panel p-4 text-ink sm:p-5">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <h2 id="needs-you-h" className="font-display text-base font-semibold text-pending-ink">
          Needs you <Badge tone="pending" aria-label={`${total} waiting`}>{total}</Badge>
        </h2>
        {approvals.length > 0 ? (
          <Link href={`${base}/approvals`} className={cn(buttonVariants({ variant: "outline", size: "sm" }))}>
            Review all approvals
          </Link>
        ) : null}
      </div>
      <ul className="mt-3 space-y-2" aria-label="Waiting for you">
        {urgent.map((a) => (
          <li key={a.id} className="flex flex-wrap items-center gap-x-3 gap-y-1 rounded-control bg-surface px-3 py-2 text-sm">
            <span className="min-w-0 flex-1 truncate font-medium">{a.summary.title}</span>
            {a.parked ? <ParkedChip /> : null}
            <span className="font-mono text-xs text-muted">{a.agent ? `@${a.agent.handle}` : "Unknown agent"}</span>
            <span className="text-xs font-medium tabular-nums text-pending-ink">{countdown(a.deadline, clock)}</span>
          </li>
        ))}
        {broken.map((a) => (
          <AccountAttentionItem key={`${a.ref.type}:${a.ref.id}`} account={a} base={base} workspaceId={wsId} />
        ))}
      </ul>
    </section>
  );
}
