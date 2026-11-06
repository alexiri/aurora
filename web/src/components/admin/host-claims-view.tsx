"use client";

import { Button } from "@/components/ui/button";
import { EmptyState, ErrorState, Loading } from "@/components/states";
import { errorMessage } from "@/lib/api/errors";
import { useDecideHostClaim, useHostClaims, type HostClaimConflict } from "@/lib/api/plugins";

export function ConflictGroup({ c }: { c: HostClaimConflict }) {
  const decide = useDecideHostClaim();
  const d = c.decision;
  return (
    <li className="space-y-2 rounded-md border border-neutral-200 p-3">
      <h2 className="font-mono text-sm font-medium">{c.group_key}</h2>
      {d ? (
        <p className="text-sm">
          Winner: {d.winner_package_id} / {d.winner_app_id}
          {d.decided_by ? ` (decided by ${d.decided_by.display_name})` : ""}
          {d.stale ? (
            <span className="ml-2 font-medium text-amber-700">
              Stale: claims changed since this decision. Choose a winner again.
            </span>
          ) : null}
        </p>
      ) : (
        <p className="text-sm text-amber-700">Unresolved</p>
      )}
      <ul className="space-y-1">
        {c.claims.map((cl) => {
          const isWinner =
            !!d && !d.stale && d.winner_package_id === cl.package_id && d.winner_app_id === cl.app_id;
          return (
            <li key={cl.claim_key + cl.package_id + cl.app_id} className="flex items-center gap-2 text-sm">
              <span>
                {cl.package_id} / {cl.app_id}
              </span>
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
            </li>
          );
        })}
      </ul>
      {decide.isError ? (
        <p role="alert" className="text-sm text-red-700">
          {errorMessage(decide.error)}
        </p>
      ) : null}
    </li>
  );
}

export function HostClaimsView() {
  const q = useHostClaims();
  return (
    <div className="space-y-4">
      <h1 className="text-xl font-semibold">Host claims</h1>
      {q.isPending ? <Loading /> : null}
      {q.isError ? <ErrorState error={q.error} onRetry={() => q.refetch()} /> : null}
      {q.data && q.data.items.length === 0 ? <EmptyState title="No host claim conflicts" /> : null}
      {q.data && q.data.items.length > 0 ? (
        <ul className="space-y-3">
          {q.data.items.map((c) => (
            <ConflictGroup key={c.group_key} c={c} />
          ))}
        </ul>
      ) : null}
    </div>
  );
}
