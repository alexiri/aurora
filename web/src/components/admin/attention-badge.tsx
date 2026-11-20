"use client";

import { CountBadge } from "@/components/attention/count-badge";
import { useAdminBadge } from "@/lib/api/attention";

/**
 * (FIX-167 Part B, D-370; P5-REFINE-18 #18, C-1082) How many things wait in Administration. Without `topic` it is the total
 * (the sidebar link, the account menu, **Overview** in the Administration rail); with `topic` it is the part that page
 * resolves (`apps`, `chat-platforms`, `people`). Amber, because the entry it sits on is its action (D-384). Mounted for
 * instance admins (the count is `null` for everyone else); with `enabled` false nothing is fetched. A failed read or a zero
 * shows nothing, never an error.
 */
export function AttentionBadge({ enabled = true, topic }: { enabled?: boolean; topic?: string }) {
  const b = useAdminBadge(enabled);
  return (
    <CountBadge
      n={topic ? b?.topics[topic] : b?.count}
      actionId="admin-nav-entry"
      testId={topic ? `admin-attention-badge-${topic}` : "admin-attention-badge"}
    />
  );
}
