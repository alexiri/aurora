"use client";

import { useState } from "react";

import { Button } from "@/components/ui/button";
import { ConfirmDialog } from "@/components/ui/confirm-dialog";
import { DataTable, Td, Th } from "@/components/ui/data-table";
import { ErrorNotice } from "@/components/ui/notice";
import { StatusDot } from "@/components/ui/status-dot";
import { intervalLabel } from "@/lib/api/addon-sources";
import {
  useCheckTemplateSource,
  useDeleteTemplateSource,
  type AgentTemplateSource,
} from "@/lib/api/agent-templates";
import { shortSha } from "@/lib/api/skill-libraries";
import { label, TEMPLATE_SOURCE_STATUS } from "@/lib/labels";
import { fmtDate } from "@/lib/utils";

import { EditSourceDialog } from "./edit-source-dialog";

/** The status of a source's last read (C-1114): "Ok", or "Can't be read" with the sanitized reason beside it. */
export function SourceStatus({ source: s }: { source: AgentTemplateSource }) {
  const tone = s.status === "ok" ? "accent" : "danger";
  const reason = s.status === "check_failed" ? s.check_failed_reason : null;
  return (
    <div className="min-w-0">
      <StatusDot tone={tone}>{label(TEMPLATE_SOURCE_STATUS, s.status)}</StatusDot>
      {reason ? <p className="mt-1 max-w-xs whitespace-normal text-xs text-danger-ink [overflow-wrap:anywhere]">{reason}</p> : null}
    </div>
  );
}

/**
 * The template repositories (C-1114): name, repository, branch or "default branch", folder, the last head as `sha7`,
 * the schedule, last checked with the status, and how many templates the source holds. **Check now**, **Edit** and
 * **Remove**. Below 900 px each row is a card.
 */
export function SourcesTable({ rows, caption }: { rows: AgentTemplateSource[]; caption: string }) {
  const check = useCheckTemplateSource();
  const del = useDeleteTemplateSource();
  const [removing, setRemoving] = useState<AgentTemplateSource | null>(null);
  const [editingId, setEditingId] = useState<string | null>(null);
  const editing = rows.find((r) => r.id === editingId) ?? null;
  const failed = check.error ?? del.error;
  return (
    <div className="space-y-3">
      <ErrorNotice error={failed} />
      <DataTable caption={caption} stackBelow={900}>
        <thead>
          <tr>
            <Th>Name</Th>
            <Th>Repository</Th>
            <Th>Branch or tag</Th>
            <Th>Folder</Th>
            <Th>Latest commit</Th>
            <Th>Check for updates</Th>
            <Th>Last checked</Th>
            <Th>Status</Th>
            <Th>Templates</Th>
            <Th>
              <span className="sr-only">Actions</span>
            </Th>
          </tr>
        </thead>
        <tbody>
          {rows.map((s) => (
            <tr key={s.id}>
              <Td cell="title" className="min-w-0 font-medium [overflow-wrap:anywhere]">
                {s.name}
              </Td>
              <Td label="Repository" className="min-w-0">
                <span className="font-mono text-xs [overflow-wrap:anywhere]">{s.repo}</span>
              </Td>
              <Td label="Branch or tag" className="font-mono text-xs">
                {s.ref ?? <span className="font-sans">Default branch</span>}
              </Td>
              <Td label="Folder" className="font-mono text-xs">
                {s.path ? s.path : <span className="font-sans">Repository root</span>}
              </Td>
              <Td label="Latest commit" className="font-mono text-xs">
                {s.last_head_sha ? shortSha(s.last_head_sha) : <span className="font-sans">—</span>}
              </Td>
              <Td label="Check for updates" className="whitespace-nowrap text-xs">
                {intervalLabel(s.update_interval)}
              </Td>
              <Td label="Last checked" className="whitespace-nowrap text-xs">
                {fmtDate(s.last_checked_at, "Never")}
              </Td>
              <Td label="Status">
                <SourceStatus source={s} />
              </Td>
              <Td label="Templates" className="text-xs">
                {s.templates.length}
              </Td>
              <Td cell="actions" className="text-right">
                <div className="flex flex-wrap justify-end gap-1">
                  <Button size="sm" variant="outline" disabled={check.isPending} onClick={() => check.mutate(s.id)} aria-label={`Check now ${s.name}`}>
                    Check now
                  </Button>
                  <Button size="sm" variant="outline" onClick={() => setEditingId(s.id)} aria-label={`Edit ${s.name}`}>
                    Edit
                  </Button>
                  <Button size="sm" variant="danger" onClick={() => setRemoving(s)} aria-label={`Remove ${s.name}`}>
                    Remove
                  </Button>
                </div>
              </Td>
            </tr>
          ))}
        </tbody>
      </DataTable>
      <EditSourceDialog source={editing} open={!!editing} onOpenChange={(o) => !o && setEditingId(null)} />
      <ConfirmDialog
        open={!!removing}
        onOpenChange={(o) => !o && setRemoving(null)}
        title={`Remove ${removing?.name ?? "repository"}?`}
        description="The templates it holds are no longer offered. Agents made from these templates keep working and do not change."
        confirmLabel="Remove repository"
        danger
        busy={del.isPending}
        onConfirm={() => removing && del.mutate(removing.id, { onSuccess: () => setRemoving(null) })}
      />
    </div>
  );
}
