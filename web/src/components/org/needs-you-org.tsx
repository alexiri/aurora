"use client";

import Link from "next/link";

import { AttentionHistory, NeedsYouCard } from "@/components/attention/needs-you-feed";
import { buttonVariants } from "@/components/ui/button";
import { useDeclineSetup } from "@/lib/api/app-setup-requests";
import { useAttentionFeed } from "@/lib/api/attention";
import { useInvitations } from "@/lib/api/org";
import { orgRoutes } from "@/lib/routes";
import { cn } from "@/lib/utils";

/**
 * (P5-REFINE-17 #16, C-1080) Needs you at the organization level, on Manage › Overview: the one feed (requests for apps,
 * chat platforms and AI keys the viewer may resolve or decline), then, not counted, the invitations still waiting for an
 * answer; **History** keeps what closed in the last 30 days.
 */
export function NeedsYouOrg({ orgId, orgSlug }: { orgId: string; orgSlug: string }) {
  const feed = useAttentionFeed("organization", orgId);
  const decline = useDeclineSetup(orgId);
  const invites = useInvitations(orgId);
  const waiting = invites.data?.items.length ?? 0;
  const items = (feed.data?.items ?? []).filter((i) => !i.own);
  return (
    <>
      <NeedsYouCard
        id="org-needs-you"
        items={items}
        count={feed.data?.count ?? 0}
        declining={decline.isPending}
        declineError={decline.error}
        onDecline={(item, message, done) => decline.mutate({ id: item.id, message }, { onSuccess: done })}
        extra={
          waiting > 0 ? (
            <li className="flex flex-wrap items-center gap-x-3 gap-y-1 rounded-control bg-surface px-3 py-2 text-sm">
              <span className="min-w-0 flex-1">
                {waiting} {waiting === 1 ? "invitation is" : "invitations are"} waiting for an answer.
              </span>
              <Link href={`${orgRoutes(orgSlug).base}/people`} className={cn(buttonVariants({ variant: "outline", size: "sm" }))}>
                See invitations
              </Link>
            </li>
          ) : null
        }
      />
      <AttentionHistory level="organization" id={orgId} />
    </>
  );
}
