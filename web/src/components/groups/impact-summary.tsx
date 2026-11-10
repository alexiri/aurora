"use client";

import { Loading } from "@/components/states";
import { Notice } from "@/components/ui/notice";
import { errorMessage } from "@/lib/api/errors";
import type { GroupImpact } from "@/lib/api/groups";

/** What a group deletion, unbinding or member removal would take away (`GET /v1/groups/{id}/impact`). */
export function ImpactSummary({
  query,
}: {
  query: { isPending: boolean; isError: boolean; error: unknown; data?: GroupImpact };
}) {
  if (query.isPending) return <Loading label="Checking what this would affect…" />;
  // The impact view needs org owner/admin: a workspace owner removing a group still may, so a
  // refusal is informational, never a blocker.
  if (query.isError) {
    return <Notice role="status">The impact could not be checked: {errorMessage(query.error)}</Notice>;
  }
  const items = query.data?.workspaces ?? [];
  if (items.length === 0) {
    return <Notice role="status">Nobody would lose access to a workspace.</Notice>;
  }
  return (
    <div className="space-y-3" aria-label="Impact">
      {items.map((w) => (
        <Notice key={w.workspace.id} tone="pending" role="status">
          <p className="font-medium">
            {w.total} {w.total === 1 ? "person" : "people"} would lose all access to {w.workspace.name}
          </p>
          {w.users_losing_access.length > 0 ? (
            <ul className="mt-1 list-disc pl-5">
              {w.users_losing_access.map((u) => (
                <li key={u.id}>
                  {u.display_name} <span className="text-muted">({u.email})</span>
                </li>
              ))}
              {w.total > w.users_losing_access.length ? (
                <li>and {w.total - w.users_losing_access.length} more</li>
              ) : null}
            </ul>
          ) : null}
          {w.schedules_disabled + w.webhooks_disabled + w.api_keys > 0 ? (
            <p className="mt-1">
              {w.schedules_disabled} schedules and {w.webhooks_disabled} webhooks would be disabled;{" "}
              {w.api_keys} API keys would stop working.
            </p>
          ) : null}
        </Notice>
      ))}
    </div>
  );
}
