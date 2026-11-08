"use client";

import { useState } from "react";

import { AdminOrgFreeze } from "@/components/freeze/freeze-sections";
import { ErrorState, Loading } from "@/components/states";
import { Card } from "@/components/ui/card";
import { PageHeader } from "@/components/ui/page-header";
import { useAdminOrgs } from "@/lib/api/freeze";

/** Instance administration, Organizations (RFE-006): the operator kill-switch, names only. */
export function AdminOrgsView() {
  const [frozenOnly, setFrozenOnly] = useState(false);
  const q = useAdminOrgs(frozenOnly);
  if (q.isPending) return <Loading />;
  if (q.isError) return <ErrorState error={q.error} onRetry={() => q.refetch()} />;
  return (
    <div className="max-w-4xl space-y-6">
      <PageHeader
        title="Organizations"
        description="Freeze an organization as the operator, for an incident such as a compromised account. Freezing gives you no access to the organization's data."
      />
      <label className="flex items-center gap-2 text-sm">
        <input type="checkbox" checked={frozenOnly} onChange={(e) => setFrozenOnly(e.target.checked)} />
        Frozen organizations only
      </label>
      {q.data.items.length === 0 ? (
        <p className="text-sm text-ink-2">{frozenOnly ? "No organization is frozen." : "No organizations."}</p>
      ) : (
        <ul className="space-y-3">
          {q.data.items.map((o) => (
            <li key={o.id}>
              <Card>
                <div className="mb-2 flex items-baseline justify-between gap-2">
                  <h2 className="text-base font-medium">{o.name}</h2>
                  <span className="font-mono text-xs text-muted">{o.slug}</span>
                </div>
                <AdminOrgFreeze orgId={o.id} name={o.name} frozen={o.frozen} />
              </Card>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
