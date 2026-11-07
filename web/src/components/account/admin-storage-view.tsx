"use client";

import { ErrorState, Loading } from "@/components/states";
import { Card } from "@/components/ui/card";
import { PageHeader } from "@/components/ui/page-header";
import { formatBytes } from "@/lib/api/attachments";
import { useAdminStorage } from "@/lib/api/org";

/** Instance storage (RFE-015): blob and attachment bytes, the top orgs and the big tables. */
export function AdminStorageView() {
  const q = useAdminStorage();
  if (q.isPending) return <Loading />;
  if (q.isError) return <ErrorState error={q.error} onRetry={() => q.refetch()} />;
  const s = q.data;
  return (
    <div className="max-w-4xl space-y-6">
      <PageHeader title="Storage" description="Where the database disk goes: attachment blobs, per organization, and the largest tables." />
      <Card>
        <dl className="grid gap-4 sm:grid-cols-3">
          <div>
            <dt className="text-sm text-ink-2">Blob bytes</dt>
            <dd className="text-xl font-semibold tabular-nums">{formatBytes(s.blob_bytes)}</dd>
            <dd className="text-xs text-muted">{s.blob_count} blobs</dd>
          </div>
          <div>
            <dt className="text-sm text-ink-2">Attachment bytes</dt>
            <dd className="text-xl font-semibold tabular-nums">{formatBytes(s.attachment_bytes)}</dd>
            <dd className="text-xs text-muted">{s.attachment_files} files</dd>
          </div>
          <div>
            <dt className="text-sm text-ink-2">Expired or deleted</dt>
            <dd className="text-xl font-semibold tabular-nums">{s.purged_files}</dd>
            <dd className="text-xs text-muted">rows kept, bytes gone</dd>
          </div>
        </dl>
      </Card>
      <Card>
        <h2 className="mb-2 text-lg font-medium">Organizations (top 50)</h2>
        {s.orgs.length === 0 ? (
          <p className="text-sm text-ink-2">No attachments stored.</p>
        ) : (
          <div className="overflow-x-auto">
            <table className="w-full text-left text-sm">
              <thead className="text-muted">
                <tr>
                  <th className="py-2 pr-4 font-medium">Organization</th>
                  <th className="py-2 pr-4 text-right font-medium">Files</th>
                  <th className="py-2 pr-4 text-right font-medium">Used</th>
                  <th className="py-2 text-right font-medium">Quota</th>
                </tr>
              </thead>
              <tbody>
                {s.orgs.map((o) => (
                  <tr key={o.org_id} className="border-t border-line">
                    <td className="py-2 pr-4">
                      <div className="font-medium">{o.name}</div>
                      <div className="text-muted">{o.slug}</div>
                    </td>
                    <td className="py-2 pr-4 text-right tabular-nums">{o.files}</td>
                    <td className="py-2 pr-4 text-right tabular-nums">{formatBytes(o.attachment_bytes)}</td>
                    <td className="py-2 text-right tabular-nums">{o.quota_bytes == null ? "Unlimited" : formatBytes(o.quota_bytes)}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
      </Card>
      <Card>
        <h2 className="mb-2 text-lg font-medium">Largest tables</h2>
        <ul className="space-y-1 text-sm">
          {s.tables.map((t) => (
            <li key={t.table} className="flex justify-between border-t border-line py-1 first:border-t-0">
              <span className="font-mono">{t.table}</span>
              <span className="tabular-nums">{formatBytes(t.bytes)}</span>
            </li>
          ))}
        </ul>
      </Card>
    </div>
  );
}
