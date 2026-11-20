"use client";

import { AttentionHistory, NeedsYouCard } from "@/components/attention/needs-you-feed";
import { useAdminDeclineRequest } from "@/lib/api/app-setup-requests";
import { useAttentionFeed } from "@/lib/api/attention";

/**
 * (FIX-167 Part B, D-370; P5-REFINE-18 #13, C-1080) Administration › Overview opens with **Needs you**: what needs the
 * instance admin, decisions first, in the one row shape every level uses: the six C-1022 kinds and the requests only an
 * instance admin can resolve (a chat platform when none is connected, an app that needs the instance), each with its
 * primary action and, on a request, **Decline** with an optional message. With nothing to show (or while loading, or on an
 * error) there is no card; **History** keeps what closed in the last 30 days.
 */
export function AttentionSection() {
  const feed = useAttentionFeed("administration", undefined);
  const decline = useAdminDeclineRequest();
  const items = (feed.data?.items ?? []).filter((i) => !i.own);
  return (
    <>
      <NeedsYouCard
        id="admin-attention"
        items={items}
        count={feed.data?.count ?? 0}
        declining={decline.isPending}
        declineError={decline.error}
        onDecline={(item, message, done) => decline.mutate({ id: item.id, message }, { onSuccess: done })}
      />
      <AttentionHistory level="administration" />
    </>
  );
}
