"use client";

import { useSearchParams } from "next/navigation";
import { useState } from "react";

import { AuditPanel, type AuditFilters } from "@/components/audit/audit-panel";
import { useAudit } from "@/lib/api/account";
import { basicFromParams, useWriteAuditParams } from "@/lib/audit-url";

/** The workspace Audit tab: the shared audit panel (D-373), with the filters in the URL. */
export function AuditTab({ wsId }: { wsId: string }) {
  const params = useSearchParams();
  const write = useWriteAuditParams();
  const [filters, setFilters] = useState<AuditFilters>(() => basicFromParams(params));
  const audit = useAudit(wsId, filters.action, filters.criticalOnly);
  return (
    <AuditPanel
      headed={false}
      title="Workspace audit entries"
      description="Who did what in this workspace, newest first."
      placeholder="Action, e.g. workspace.update"
      feed={audit}
      filters={filters}
      onFilters={(n) => {
        setFilters(n);
        write(n);
      }}
    />
  );
}
