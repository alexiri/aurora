"use client";

import { useSearchParams } from "next/navigation";
import { useState } from "react";

import { AuditPanel, type AuditFilters } from "@/components/audit/audit-panel";
import { EmergencyHistory } from "@/components/emergency/emergency-history";
import { useOrgAudit } from "@/lib/api/org";
import { basicFromParams, useWriteAuditParams } from "@/lib/audit-url";
import { auditFilterGroups } from "@/lib/labels";

export function OrgAuditView({ orgId }: { orgId: string }) {
  const params = useSearchParams();
  const write = useWriteAuditParams();
  const [filters, setFilters] = useState<AuditFilters>(() => basicFromParams(params));
  const audit = useOrgAudit(orgId, filters.action, filters.criticalOnly);
  return (
    <div className="space-y-6">
      <AuditPanel
        title="Audit log"
        description="Who did what in this organization, newest first."
        actions={auditFilterGroups("organization")}
        feed={audit}
        filters={filters}
        onFilters={(n) => {
          setFilters(n);
          write(n);
        }}
      />
      {/* (FIX-236, C-1164) Emergency access is its own list here: the organization's every grant, with the workspace named. */}
      <EmergencyHistory orgId={orgId} withWorkspace />
    </div>
  );
}
