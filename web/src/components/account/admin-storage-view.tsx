"use client";

import { ErrorState, Loading } from "@/components/states";
import { Card } from "@/components/ui/card";
import { DataTable, Td, Th } from "@/components/ui/data-table";
import { PageHeader } from "@/components/ui/page-header";
import { ItemList, ListRow, Section } from "@/components/ui/section";
import { formatBytes } from "@/lib/api/attachments";
import { StorageFreshness } from "@/components/storage-freshness";
import { useAdminStorage, useRefreshAdminStorage } from "@/lib/api/org";

/** Instance storage (RFE-015): blob and attachment bytes, the top orgs and the big tables. */
export function AdminStorageView() {
  const q = useAdminStorage();
  const refresh = useRefreshAdminStorage();
  if (q.isPending) return <Loading />;
  if (q.isError) return <ErrorState error={q.error} onRetry={() => q.refetch()} />;
  const s = q.data;
  return (
    <div className="max-w-[1160px] space-y-6">
      <PageHeader
        title="Storage"
        description="Where the database disk goes: attachment blobs, per organization, and the largest tables."
        className="mb-6"
      />
      <StorageFreshness computedAt={s.computed_at} refresh={refresh} />
      <Card>
        <dl className="grid gap-4 sm:grid-cols-3">
          <div>
            <dt className="text-xs font-semibold uppercase tracking-[0.06em] text-muted">Blob bytes</dt>
            <dd className="mt-1 font-display text-2xl font-semibold tabular-nums text-ink">{formatBytes(s.blob_bytes)}</dd>
            <dd className="text-xs text-muted">{s.blob_count} blobs</dd>
          </div>
          <div>
            <dt className="text-xs font-semibold uppercase tracking-[0.06em] text-muted">Attachment bytes</dt>
            <dd className="mt-1 font-display text-2xl font-semibold tabular-nums text-ink">{formatBytes(s.attachment_bytes)}</dd>
            <dd className="text-xs text-muted">{s.attachment_files} files</dd>
          </div>
          <div>
            <dt className="text-xs font-semibold uppercase tracking-[0.06em] text-muted">Expired or deleted</dt>
            <dd className="mt-1 font-display text-2xl font-semibold tabular-nums text-ink">{s.purged_files}</dd>
            <dd className="text-xs text-muted">rows kept, bytes gone</dd>
          </div>
        </dl>
      </Card>
      <Section id="top-orgs" title="Organizations (top 50)">
        {s.orgs.length === 0 ? (
          <p className="text-sm text-ink-2">No attachments stored.</p>
        ) : (
          <DataTable caption="Organizations by attachment bytes">
            <thead>
              <tr>
                <Th>Organization</Th>
                <Th className="text-right">Files</Th>
                <Th className="text-right">Used</Th>
                <Th className="text-right">Quota</Th>
              </tr>
            </thead>
            <tbody>
              {s.orgs.map((o) => (
                <tr key={o.org_id}>
                  <Td>
                    <div className="font-medium">{o.name}</div>
                    <div className="font-mono text-xs text-muted">{o.slug}</div>
                  </Td>
                  <Td className="text-right tabular-nums">{o.files}</Td>
                  <Td className="text-right tabular-nums">{formatBytes(o.attachment_bytes)}</Td>
                  <Td className="text-right tabular-nums">{o.quota_bytes == null ? "Unlimited" : formatBytes(o.quota_bytes)}</Td>
                </tr>
              ))}
            </tbody>
          </DataTable>
        )}
      </Section>
      <Section id="tables" title="Largest tables">
        <ItemList aria-label="Largest tables">
          {s.tables.map((t) => (
            <ListRow key={t.table}>
              <span className="font-mono text-sm text-ink">{t.table}</span>
              <span className="ml-auto text-sm tabular-nums text-ink-2">{formatBytes(t.bytes)}</span>
            </ListRow>
          ))}
        </ItemList>
      </Section>
    </div>
  );
}
