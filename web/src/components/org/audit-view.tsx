"use client";

import { useState } from "react";

import { AuditPanel, type AuditFilters } from "@/components/audit/audit-panel";
import { useOrgAudit } from "@/lib/api/org";

export function OrgAuditView({ orgId }: { orgId: string }) {
  const [filters, setFilters] = useState<AuditFilters>({ action: "", criticalOnly: false });
  const audit = useOrgAudit(orgId, filters.action, filters.criticalOnly);
  return (
    <AuditPanel
      title="Audit log"
      description="Who did what in this organization, newest first."
      placeholder="Action, e.g. member.update"
      feed={audit}
      filters={filters}
      onFilters={setFilters}
    />
  );
}
