"use client";

import { useSearchParams } from "next/navigation";
import { useState } from "react";

import { AuditPanel, type AuditFilters } from "@/components/audit/audit-panel";
import { useAccess, useAudit, useWorkspaceKeys } from "@/lib/api/account";
import { useAgents } from "@/lib/api/queries";
import { basicFromParams, useWriteAuditParams } from "@/lib/audit-url";
import { auditFilterGroups } from "@/lib/labels";

/** The workspace Audit tab: the shared audit panel (D-373), with the filters in the URL. */
export function AuditTab({ wsId }: { wsId: string }) {
  const params = useSearchParams();
  const write = useWriteAuditParams();
  const [filters, setFilters] = useState<AuditFilters>(() => basicFromParams(params));
  const audit = useAudit(wsId, filters.action, filters.criticalOnly);
  // (workspace-3) Names for what an entry points at, from what this workspace knows now. A deleted thing has no name here.
  const access = useAccess(wsId);
  const keys = useWorkspaceKeys(wsId);
  const agents = useAgents(wsId);
  const names = new Map<string, string>();
  for (const p of access.data?.items ?? []) names.set(p.user_id, p.display_name || p.email);
  for (const k of keys.data?.items ?? []) names.set(k.id, k.name);
  for (const a of agents.data?.items ?? []) names.set(a.id, a.name);
  return (
    <AuditPanel
      headed={false}
      title="Workspace audit entries"
      description="Who changed what in this workspace."
      actions={auditFilterGroups("workspace")}
      targetName={(e) => (e.target_id ? (names.get(e.target_id) ?? null) : null)}
      feed={audit}
      filters={filters}
      onFilters={(n) => {
        setFilters(n);
        write(n);
      }}
    />
  );
}
