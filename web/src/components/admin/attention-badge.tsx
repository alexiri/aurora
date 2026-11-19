"use client";

import { AttentionChip } from "@/components/ui/attention-chip";
import { useAdminAttention } from "@/lib/api/attention";

/**
 * (FIX-167 Part B, D-370, C-1022) How many things wait in Administration, on the Administration entry. Amber, because
 * something needs the viewer (D-384): the entry it sits on is its action, and the list of what waits is one click away.
 * Mounted for instance admins only (the route answers 403 to everyone else); with `enabled` false nothing is fetched.
 * A failed fetch or an empty list shows nothing, never an error.
 */
export function AttentionBadge({ enabled = true }: { enabled?: boolean }) {
  const q = useAdminAttention(enabled);
  const n = enabled && !q.isError ? (q.data?.items.length ?? 0) : 0;
  if (n === 0) return null;
  return (
    <AttentionChip actionId="admin-nav-entry" data-testid="admin-attention-badge" className="tabular-nums">
      {n}
      <span className="sr-only"> need your attention</span>
    </AttentionChip>
  );
}
