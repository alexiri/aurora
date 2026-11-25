"use client";

// (FIX-236, C-1164) The "Emergency access" history: who opened it, why, from–to and how it ended.
// One list for both places — the workspace's People page (`wsId`) and the organization's audit
// topic (`orgId`, with the workspace named) — each from its own GET route, newest first.

import { Badge } from "@/components/ui/badge";
import { ItemList, ListRow, Section } from "@/components/ui/section";
import { ErrorState, Loading } from "@/components/states";
import { useOrgEmergencyAccess, useWorkspaceEmergencyAccess, type EmergencyAccessRow } from "@/lib/api/emergency";
import { emergencyEndText, emergencySpanText } from "@/lib/emergency";
import { formatTime } from "@/lib/format-date";
import { personName } from "@/lib/workspace-name";

/** One row of the history. `workspaceName` comes with the organization's list. */
function EmergencyRow({ row, workspaceName }: { row: EmergencyAccessRow; workspaceName?: string }) {
  return (
    <ListRow data-testid="emergency-history-row">
      <div className="min-w-0 flex-1">
        <p className="min-w-0">
          <span className="font-medium">{personName(row.by.display_name)}</span>
          {workspaceName ? <span className="ml-2 text-sm text-muted">{workspaceName}</span> : null}
          {row.active ? (
            <Badge tone="accent" className="ml-2" data-testid="emergency-history-active">
              Until {formatTime(row.expires_at)}
            </Badge>
          ) : null}
        </p>
        <p className="text-sm text-ink-2">Reason: {row.reason}</p>
        <p className="text-xs text-muted">
          {/* (FIX-236, review LOW-11) An expired-not-yet-swept row has `ended_at: null`; the
              span falls back to `expires_at` instead of repeating the open time. */}
          From {emergencySpanText(row.opened_at, row.ended_at ?? row.expires_at)}
          {row.active ? null : <> · {emergencyEndText(row)}</>}
        </p>
      </div>
    </ListRow>
  );
}

/**
 * The history section. The caller decides whether the viewer may read the list (the workspace's
 * answers to own access or `can_administer`, the organization's to owners and admins); a refusal
 * shows as its own error state with a retry.
 */
export function EmergencyHistory({ wsId, orgId, withWorkspace = false }: { wsId?: string; orgId?: string; withWorkspace?: boolean }) {
  const list = useWorkspaceEmergencyAccess(wsId, !!wsId);
  const orgList = useOrgEmergencyAccess(orgId, !!orgId);
  const q = wsId ? list : orgList;
  return (
    <Section
      id="emergency-history"
      title="Emergency access"
      description={
        withWorkspace
          ? "Every time emergency read access was opened in this organization, and how it ended."
          : "Every time emergency read access was opened here, and how it ended."
      }
    >
      {q.isPending ? (
        <Loading />
      ) : q.isError ? (
        <ErrorState error={q.error} onRetry={() => void q.refetch()} />
      ) : (q.data?.items.length ?? 0) === 0 ? (
        <p className="text-sm text-muted" data-testid="emergency-history-empty">
          Never opened.
        </p>
      ) : (
        <ItemList aria-label="Emergency access history">
          {(q.data?.items ?? []).map((row) => (
            <EmergencyRow key={row.id} row={row} workspaceName={withWorkspace ? row.workspace.name : undefined} />
          ))}
        </ItemList>
      )}
      {q.data?.truncated ? <p className="text-xs text-muted">Older rows are not shown.</p> : null}
    </Section>
  );
}
