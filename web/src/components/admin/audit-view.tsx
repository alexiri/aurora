"use client";

import { useSearchParams } from "next/navigation";
import { useState } from "react";

import { AuditPanel } from "@/components/audit/audit-panel";
import { inputClass } from "@/components/ui/field";
import { Segmented } from "@/components/ui/segmented";
import { useInstanceAudit, type InstanceAuditFilter } from "@/lib/api/admin-audit";
import { useAdminOrgs } from "@/lib/api/freeze";
import { instanceFromParams, useWriteAuditParams } from "@/lib/audit-url";

/** Instance administration, Audit (FIX-125): what instance admins did, and optionally every organization's rows. */
export function AdminAuditView() {
  const params = useSearchParams();
  const write = useWriteAuditParams();
  // The first values come from the URL (a shared link opens the same view); every change is written back to it.
  const [f, setFilter] = useState<InstanceAuditFilter>(() => instanceFromParams(params));
  const setF = (n: InstanceAuditFilter) => {
    setFilter(n);
    write(n);
  };
  const audit = useInstanceAudit(f);
  const orgs = useAdminOrgs(false);
  const all = f.scope === "all";
  return (
    <AuditPanel
      title="Audit log"
      description="What instance administrators did: plugin approvals, host claims, trusted keys, connectors, runners, placement and freezes. Switch to all organizations to search every row."
      placeholder="Action prefix, e.g. plugin."
      feed={audit}
      filters={{ action: f.action, criticalOnly: f.criticalOnly }}
      onFilters={(n) => setF({ ...f, ...n })}
      showOrg={all}
      extraFilters={
        <>
          <Segmented
            label="Scope"
            value={f.scope}
            options={[
              { value: "instance", label: "Instance" },
              { value: "all", label: "All organizations" },
            ]}
            onChange={(scope) => setF({ ...f, scope, org: scope === "all" ? f.org : "" })}
          />
          {all ? (
            <select aria-label="Organization" className={`${inputClass} max-w-56`} value={f.org} onChange={(e) => setF({ ...f, org: e.target.value })}>
              <option value="">Every organization</option>
              {orgs.data?.items.map((o) => (
                <option key={o.id} value={o.id}>
                  {o.name}
                </option>
              ))}
            </select>
          ) : null}
          <label className="flex items-center gap-2 text-sm">
            From
            <input
              type="date"
              aria-label="From date"
              className={`${inputClass} w-auto`}
              value={f.since}
              onChange={(e) => setF({ ...f, since: e.target.value })}
            />
          </label>
          <label className="flex items-center gap-2 text-sm">
            To
            <input
              type="date"
              aria-label="To date"
              className={`${inputClass} w-auto`}
              value={f.until}
              onChange={(e) => setF({ ...f, until: e.target.value })}
            />
          </label>
        </>
      }
    />
  );
}
