"use client";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Notice } from "@/components/ui/notice";
import { PageHeader } from "@/components/ui/page-header";
import { ItemList, ListRow } from "@/components/ui/section";
import { EmptyState, ErrorState, Loading } from "@/components/states";
import { errorMessage } from "@/lib/api/errors";
import { useDecideHostClaim, useHostClaims, type HostClaimConflict } from "@/lib/api/plugins";

export function ConflictGroup({ c }: { c: HostClaimConflict }) {
  const decide = useDecideHostClaim();
  const d = c.decision;
  return (
    <li className="space-y-3 rounded-card border border-line bg-surface p-4 shadow-card sm:p-5">
      <div className="flex flex-wrap items-center gap-x-3 gap-y-1">
        <h2 className="break-all font-mono text-sm font-medium text-ink">{c.group_key}</h2>
        {!d ? <Badge tone="pending">Unresolved</Badge> : null}
      </div>
      {d ? (
        <p className="text-sm text-ink-2">
          Winner: {d.winner_package_id} / {d.winner_app_id}
          {d.decided_by ? ` (decided by ${d.decided_by.display_name})` : ""}
          {d.stale ? (
            <span className="ml-2 font-medium text-pending-ink">
              Stale: claims changed since this decision. Choose a winner again.
            </span>
          ) : null}
        </p>
      ) : (
        <p className="sr-only">Unresolved</p>
      )}
      <ItemList aria-label={`Claims for ${c.group_key}`} className="shadow-none">
        {c.claims.map((cl) => {
          const isWinner =
            !!d && !d.stale && d.winner_package_id === cl.package_id && d.winner_app_id === cl.app_id;
          return (
            <ListRow
              key={cl.claim_key + cl.package_id + cl.app_id}
              actions={
                <Button
                  size="sm"
                  variant="outline"
                  disabled={decide.isPending || isWinner}
                  aria-label={`Pick ${cl.package_id} / ${cl.app_id} as winner for ${c.group_key}`}
                  onClick={() =>
                    decide.mutate({
                      group_key: c.group_key,
                      winner_package_id: cl.package_id,
                      winner_app_id: cl.app_id,
                      fingerprint: c.fingerprint,
                    })
                  }
                >
                  {isWinner ? "Winner" : "Pick winner"}
                </Button>
              }
            >
              <span className="text-sm text-ink">
                {cl.package_id} / {cl.app_id}
              </span>
            </ListRow>
          );
        })}
      </ItemList>
      {decide.isError ? (
        <Notice tone="danger" role="alert">
          {errorMessage(decide.error)}
        </Notice>
      ) : null}
    </li>
  );
}

export function HostClaimsView() {
  const q = useHostClaims();
  return (
    <div className="max-w-[1160px] space-y-4">
      <PageHeader title="Host claims" description="Apps that claim the same host. Pick which one wins." className="mb-2" />
      {q.isPending ? <Loading /> : null}
      {q.isError ? <ErrorState error={q.error} onRetry={() => q.refetch()} /> : null}
      {q.data && q.data.items.length === 0 ? <EmptyState title="No host claim conflicts" /> : null}
      {q.data && q.data.items.length > 0 ? (
        <ul className="space-y-4">
          {q.data.items.map((c) => (
            <ConflictGroup key={c.group_key} c={c} />
          ))}
        </ul>
      ) : null}
    </div>
  );
}
