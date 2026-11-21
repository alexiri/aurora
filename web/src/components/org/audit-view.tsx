"use client";

import { useSearchParams } from "next/navigation";
import { useState } from "react";

import { AuditPanel, type AuditFilters } from "@/components/audit/audit-panel";
import { useOrgAudit } from "@/lib/api/org";
import { basicFromParams, useWriteAuditParams } from "@/lib/audit-url";
import { auditFilterGroups } from "@/lib/labels";

export function OrgAuditView({ orgId }: { orgId: string }) {
  const params = useSearchParams();
  const write = useWriteAuditParams();
  const [filters, setFilters] = useState<AuditFilters>(() => basicFromParams(params));
  const audit = useOrgAudit(orgId, filters.action, filters.criticalOnly);
  return (
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
  );
}
