"use client";

import { useSearchParams } from "next/navigation";
import { useState } from "react";

import { AuditPanel } from "@/components/audit/audit-panel";
import { DateField } from "@/components/ui/date-field";
import { inputClass } from "@/components/ui/field";
import { Segmented } from "@/components/ui/segmented";
import { Switch } from "@/components/ui/switch";
import { isSystemAudit } from "@/lib/labels";
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
  // (P5-REFINE-18 #11, admin-10) The system's own install and sync events are hidden until asked for, and the count says
  // so. They are told apart in the rows loaded so far; C-1088's server filter will cover every row.
  const [showSystem, setShowSystem] = useState(false);
  const loaded = audit.data?.pages.flatMap((p) => p.items) ?? [];
  const hidden = loaded.filter(isSystemAudit).length;
  const feed = {
    ...audit,
    data: audit.data
      ? { pages: audit.data.pages.map((p) => ({ ...p, items: showSystem ? p.items : p.items.filter((e) => !isSystemAudit(e)) })) }
      : undefined,
  };
  return (
    <AuditPanel
      title="Audit log"
      description="What administrators did."
      placeholder="Action prefix, e.g. plugin."
      targetName={() => null}
      areas
      afterList={
        hidden > 0 || showSystem ? (
          <div className="flex flex-wrap items-center gap-3 text-sm text-ink-2" data-testid="system-events">
            <Switch id="audit-system" checked={showSystem} onCheckedChange={setShowSystem} />
            <label htmlFor="audit-system">Show system events</label>
            <span>{showSystem ? `Showing ${hidden} system install events.` : `${hidden} system install events are hidden.`}</span>
          </div>
        ) : null
      }
      feed={feed}
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
            <DateField aria-label="From date" value={f.since} onChange={(v) => setF({ ...f, since: v })} />
          </label>
          <label className="flex items-center gap-2 text-sm">
            To
            <DateField aria-label="To date" value={f.until} onChange={(v) => setF({ ...f, until: v })} />
          </label>
        </>
      }
    />
  );
}
