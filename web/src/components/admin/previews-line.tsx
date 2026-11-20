"use client";

import { Card } from "@/components/ui/card";
import { formatWhen } from "@/lib/format-date";
import { StatusDot } from "@/components/ui/status-dot";
import { useAdminOverview } from "@/lib/api/org";

/**
 * (FIX-148, C-841) The "Previews" line of the admin overview (`docs/operator/previews.md`): the
 * relay replicas the API has seen, shown only while previews are enabled (the overview omits
 * `previews` otherwise) — nothing while loading, on error, or when off.
 */
export function PreviewsLine() {
  const overview = useAdminOverview();
  const previews = overview.data?.previews;
  if (!previews) return null;
  const n = previews.relay_replicas;
  return (
    <Card className="space-y-1 p-3 text-sm sm:p-3" data-testid="previews-line">
      <div className="flex flex-wrap items-center gap-x-3 gap-y-1">
        <span className="font-medium text-ink">Previews</span>
        <StatusDot tone={n > 0 ? "accent" : "pending"}>{n > 0 ? "Relay up" : "No relay seen"}</StatusDot>
        <span className="min-w-0 break-words text-ink-2">
          {n === 0
            ? "No relay replica has reported in the last 15 minutes."
            : `${n} relay ${n === 1 ? "replica" : "replicas"} seen` +
              (previews.last_seen_at ? `, last ${formatWhen(previews.last_seen_at)}` : "")}
        </span>
      </div>
    </Card>
  );
}
