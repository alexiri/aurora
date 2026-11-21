"use client";

import Link from "next/link";

import { SetupRequestItem } from "@/components/overview/setup-request-row";
import { AttentionChip } from "@/components/ui/attention-chip";
import { buttonVariants } from "@/components/ui/button";
import { useOrgSetupRequests } from "@/lib/api/app-setup-requests";
import { type AttentionItem, useAttentionFeed, useWorkspaceCounts } from "@/lib/api/attention";
import { useOrg } from "@/lib/api/queries";
import { whenInSentence } from "@/lib/format-date";
import { cn } from "@/lib/utils";

/**
 * One row of the workspace attention feed (C-1080, D-417): the sentence, the detail, how long it has waited and the one
 * action. The viewer's own open request reads "Waiting for an admin" and has no action; it is never counted.
 */
function AttentionRow({ item }: { item: AttentionItem }) {
  return (
    <li className="flex flex-wrap items-center gap-x-3 gap-y-1 rounded-control bg-surface px-3 py-2 text-sm" data-kind={item.kind}>
      <span className="min-w-0 flex-1">
        <span className="block font-medium">{item.title}</span>
        <span className="block text-xs text-muted">
          {item.detail} <span className="tabular-nums">Since {whenInSentence(item.since)}.</span>
        </span>
      </span>
      {item.action ? (
        <Link href={item.action.href} className={cn(buttonVariants({ variant: "outline", size: "sm" }))}>
          {item.action.label}
        </Link>
      ) : null}
    </li>
  );
}

/**
 * The Overview's **Needs you** card (the Overview board, C-945; D-417): what waits for a person, in one place, read from the
 * workspace's attention feed (approvals they can decide, access requests, accounts that stopped working, their own open
 * requests). The number in the heading is the number on the rail's Overview entry (C-1082, the one shared count hook).
 * For an organization's owners and admins the organization's setup requests sit below, with Set up and Not now.
 * Nothing when nothing waits.
 */
export function NeedsYou({ wsId, base, orgId }: { wsId: string; base: string; orgId?: string }) {
  const feed = useAttentionFeed("workspace", wsId);
  const { count } = useWorkspaceCounts(wsId);
  // (P5-POLISH-4, D-386) App setup requests: for the organization's owners and admins only.
  const org = useOrg(orgId);
  const isOrgAdmin = org.data?.my_role === "owner" || org.data?.my_role === "admin";
  const setups = useOrgSetupRequests(orgId, isOrgAdmin);
  const requests = isOrgAdmin ? (setups.data ?? []) : [];
  const orgBase = base.split("/w/")[0]!;
  const items = feed.isError ? [] : (feed.data?.items ?? []);
  if (items.length === 0 && requests.length === 0) return null;
  const hasApprovals = items.some((i) => i.kind === "approval");

  return (
    <section aria-labelledby="needs-you-h" className="rounded-card border border-pending-line bg-pending-panel p-4 text-ink sm:p-5">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <h2 id="needs-you-h" className="font-display text-base font-semibold text-pending-ink">
          Needs you{" "}
          {count > 0 ? (
            <AttentionChip actionId="needs-you-list" role="img" aria-label={`${count} waiting`}>
              {count}
            </AttentionChip>
          ) : null}
        </h2>
        {hasApprovals ? (
          <Link href={`${base}/approvals`} className={cn(buttonVariants({ variant: "outline", size: "sm" }))}>
            Review all approvals
          </Link>
        ) : null}
      </div>
      {items.length > 0 ? (
        <ul id="needs-you-list" className="mt-3 space-y-2" aria-label="Waiting for you">
          {items.map((i) => (
            <AttentionRow key={i.id} item={i} />
          ))}
        </ul>
      ) : null}
      {orgId && requests.length > 0 ? (
        <>
          <h3 className="mt-4 text-sm font-semibold text-pending-ink">For your organization</h3>
          <ul className="mt-2 space-y-2" aria-label="Waiting for your organization">
            {requests.map((r) => (
              <SetupRequestItem key={r.id} request={r} orgBase={orgBase} orgId={orgId} />
            ))}
          </ul>
        </>
      ) : null}
    </section>
  );
}
