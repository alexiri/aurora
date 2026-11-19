"use client";

import { Badge } from "@/components/ui/badge";
import { useAdminAttention } from "@/lib/api/attention";

/**
 * (FIX-167 Part B, D-370, C-1022) How many things wait in Administration, as a `Badge` on the Administration entry.
 * Mounted for instance admins only (the route answers 403 to everyone else); with `enabled` false nothing is fetched.
 * A failed fetch or an empty list shows nothing, never an error.
 */
export function AttentionBadge({ enabled = true }: { enabled?: boolean }) {
  const q = useAdminAttention(enabled);
  const n = enabled && !q.isError ? (q.data?.items.length ?? 0) : 0;
  if (n === 0) return null;
  return (
    <Badge tone="pending" data-testid="admin-attention-badge" className="tabular-nums">
      {n}
      <span className="sr-only"> need your attention</span>
    </Badge>
  );
}
