"use client";

import { ErrorState, Loading } from "@/components/states";
import { Badge } from "@/components/ui/badge";
import { DataTable, Td, Th } from "@/components/ui/data-table";
import { Meter } from "@/components/ui/meter";
import { Notice } from "@/components/ui/notice";
import { PageHeader } from "@/components/ui/page-header";
import { Section } from "@/components/ui/section";
import { formatBytes } from "@/lib/api/attachments";
import { StorageFreshness } from "@/components/storage-freshness";
import { useOrgStorage, useRefreshOrgStorage } from "@/lib/api/org";

const NOTICE: Record<string, { tone: "neutral" | "pending" | "danger"; label: string }> = {
  clear: { tone: "neutral", label: "OK" },
  warning: { tone: "pending", label: "80 % used" },
  full: { tone: "danger", label: "Full" },
};

/** Usage bar with text, so the state never depends on colour alone. */
export function UsageBar({ used, quota }: { used: number; quota: number | null | undefined }) {
  if (quota == null) return <p className="text-sm text-ink-2">{formatBytes(used)} used, no limit.</p>;
  const pct = quota > 0 ? Math.min(100, Math.round((used / quota) * 100)) : 100;
  return (
    <div>
      <Meter
        role="progressbar"
        aria-label="Attachment storage used"
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={pct}
        ratio={pct / 100}
        tone={pct >= 100 ? "danger" : pct >= 80 ? "pending" : "accent-ink"}
      />
      <p className="mt-1 text-sm text-ink-2">
        {formatBytes(used)} of {formatBytes(quota)} ({pct} %)
      </p>
    </div>
  );
}

export function OrgStorageView({ orgId }: { orgId: string }) {
  const storage = useOrgStorage(orgId);
  const refresh = useRefreshOrgStorage(orgId);
  if (storage.isPending) return <Loading />;
  if (storage.isError) return <ErrorState error={storage.error} onRetry={() => storage.refetch()} />;
  const s = storage.data;
  const notice = NOTICE[s.notice] ?? NOTICE.clear;
  return (
    <div className="max-w-page space-y-8">
      <PageHeader title="Storage" description="Attachment bytes used by this organization, by workspace and direction." className="mb-6" />
      <StorageFreshness computedAt={s.computed_at} refresh={refresh} />
      <Section id="usage" title="Usage" actions={<Badge tone={notice.tone}>{notice.label}</Badge>} card>
        <div className="space-y-3">
          <UsageBar used={s.used_bytes} quota={s.quota_bytes} />
          {s.notice === "full" ? (
            <Notice tone="danger" role="alert">
              Your organization&apos;s attachment storage is full. Uploads and files sent by agents are refused until space is freed or the
              quota is raised.
            </Notice>
          ) : null}
          <p className="text-sm text-ink-2">
            {s.files} files hold bytes, {s.purged_files} expired or deleted.
          </p>
        </div>
      </Section>
      <Section id="by-workspace" title="By workspace">
        {s.by_workspace.length === 0 ? (
          <p className="text-sm text-ink-2">No attachments stored.</p>
        ) : (
          <DataTable caption="Attachment bytes by workspace">
            <thead>
              <tr>
                <Th>Workspace</Th>
                <Th>Direction</Th>
                <Th className="text-right">Files</Th>
                <Th className="text-right">Size</Th>
              </tr>
            </thead>
            <tbody>
              {s.by_workspace.map((r) => (
                <tr key={`${r.workspace_id}-${r.direction}`}>
                  <Td>{r.workspace_name}</Td>
                  <Td>{r.direction === "in" ? "Uploaded by people" : "Sent by agents"}</Td>
                  <Td className="text-right tabular-nums">{r.files}</Td>
                  <Td className="text-right tabular-nums">{formatBytes(r.bytes)}</Td>
                </tr>
              ))}
            </tbody>
          </DataTable>
        )}
      </Section>
    </div>
  );
}
