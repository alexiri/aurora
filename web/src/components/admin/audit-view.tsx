"use client";

import { useSearchParams } from "next/navigation";
import { useState } from "react";

import { AuditPanel } from "@/components/audit/audit-panel";
import { DateField } from "@/components/ui/date-field";
import { inputClass } from "@/components/ui/field";
import { Segmented } from "@/components/ui/segmented";
import { Switch } from "@/components/ui/switch";
import { useInstanceAudit, type InstanceAuditFilter } from "@/lib/api/admin-audit";
import { useAdminOrgs } from "@/lib/api/freeze";
import { instanceFromParams, useWriteAuditParams } from "@/lib/audit-url";
import { auditFilterGroups, type AuditFilterGroup } from "@/lib/labels";

/** (admin-10) Whole areas of Administration, by the action prefix the instance list takes, in words. */
const ADMIN_AREAS: AuditFilterGroup = {
  group: "Whole areas",
  options: [
    { value: "instance_settings.", label: "Instance settings" },
    { value: "placement.", label: "New users" },
    { value: "plugin.", label: "Apps" },
    { value: "connector.", label: "Chat platforms" },
    { value: "runner.", label: "Runners" },
  ],
};

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
  const [showSystem, setShowSystem] = useState(false);
  const audit = useInstanceAudit(f, showSystem);
  const orgs = useAdminOrgs(false);
  const all = f.scope === "all";
  // (P5-REFINE-18 #11, admin-10, C-1088) The server leaves the system's install events out and counts them in
  // `hidden_system` (over every row, not the page); with the switch on, the count it last gave is remembered.
  const hiddenNow = audit.data?.pages[0]?.hidden_system;
  const [hidden, setHidden] = useState(0);
  if (typeof hiddenNow === "number" && hiddenNow !== hidden) setHidden(hiddenNow);
  const feed = audit;
  return (
    <AuditPanel
      title="Audit log"
      description="What administrators did."
      actions={[ADMIN_AREAS, ...auditFilterGroups("instance")]}
      targetName={() => null}
      areas
      afterList={
        hidden > 0 || showSystem ? (
          <div className="flex flex-wrap items-center gap-3 text-sm text-ink-2" data-testid="system-events">
            <Switch id="audit-system" checked={showSystem} onCheckedChange={setShowSystem} />
            <label htmlFor="audit-system">Show system events</label>
            <span>{showSystem ? "Showing the system install events too." : `${hidden} system install events are hidden.`}</span>
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
            <DateField format="app" aria-label="From date" value={f.since} onChange={(v) => setF({ ...f, since: v })} />
          </label>
          <label className="flex items-center gap-2 text-sm">
            To
            <DateField format="app" aria-label="To date" value={f.until} onChange={(v) => setF({ ...f, until: v })} />
          </label>
        </>
      }
    />
  );
}
