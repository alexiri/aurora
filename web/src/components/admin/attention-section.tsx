"use client";

import type { ReactNode } from "react";

import { AttentionHistory, NeedsYouCard } from "@/components/attention/needs-you-feed";
import { useAdminDeclineRequest } from "@/lib/api/app-setup-requests";
import { useAttentionFeed } from "@/lib/api/attention";

/**
 * (FIX-167 Part B, D-370; P5-REFINE-18 #13, C-1080; P5-REFINE-20 setup-5) Administration's **Needs you**: what needs the
 * instance admin, decisions first, in the one row shape every level uses: the six C-1022 kinds and the requests only an
 * instance admin can resolve (a chat platform when none is connected, an app that needs the instance), each with its
 * primary action and, on a request, **Decline** with an optional message. `extra` rows (apps people cannot sign in to,
 * e-mail not set up: read from the setup status) join the same list, uncounted. With nothing to show (or while loading, or
 * on an error) there is no card. **History** (what closed in the last 30 days) follows it unless `history` is false and
 * the page puts it at its end.
 */
export function AttentionSection({ extra, history = true }: { extra?: ReactNode; history?: boolean }) {
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
        extra={extra}
      />
      {history ? <AttentionHistory level="administration" /> : null}
    </>
  );
}
