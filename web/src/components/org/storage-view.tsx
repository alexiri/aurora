"use client";

import { useState, type FormEvent } from "react";

import { ErrorState, Loading } from "@/components/states";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { Field, inputClass } from "@/components/ui/field";
import { PageHeader } from "@/components/ui/page-header";
import { formatBytes } from "@/lib/api/attachments";
import { errorMessage } from "@/lib/api/errors";
import { useOrgStorage, usePutAttachmentLimits, useRetention } from "@/lib/api/org";

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
      <div
        role="progressbar"
        aria-label="Attachment storage used"
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={pct}
        className="h-2 w-full overflow-hidden rounded-full bg-subtle"
      >
        <div className={pct >= 100 ? "h-full bg-danger-ink" : pct >= 80 ? "h-full bg-pending-ink" : "h-full bg-accent-ink"} style={{ width: `${pct}%` }} />
      </div>
      <p className="mt-1 text-sm text-ink-2">
        {formatBytes(used)} of {formatBytes(quota)} ({pct} %)
      </p>
    </div>
  );
}

function toNumber(t: string, min: number, max: number): number | undefined | "invalid" {
  const v = t.trim();
  if (v === "") return undefined;
  const n = /^\d+$/.test(v) ? Number(v) : NaN;
  return n >= min && n <= max ? n : "invalid";
}

/** Attachment retention window and quota (RFE-015), validated against the instance maximums. */
function LimitsForm({ orgId }: { orgId: string }) {
  const current = useRetention(orgId);
  const put = usePutAttachmentLimits(orgId);
  const [days, setDays] = useState<string | null>(null);
  const [quota, setQuota] = useState<string | null>(null);
  const [clientErr, setClientErr] = useState<{ days?: string; quota?: string }>({});
  const [saved, setSaved] = useState(false);
  if (current.isPending || !current.data) return null;
  const d = current.data;
  const daysMax = d.attachments_days_max ?? null;
  const quotaMax = d.attachment_quota_mib_max ?? null;
  const daysValue = days ?? d.attachments_days?.toString() ?? "";
  const quotaValue = quota ?? d.attachment_quota_mib?.toString() ?? "";

  function submit(e: FormEvent) {
    e.preventDefault();
    setSaved(false);
    const nd = toNumber(daysValue, 7, 3650);
    const nq = toNumber(quotaValue, 0, 1 << 30);
    const err: { days?: string; quota?: string } = {};
    if (nd === "invalid") err.days = "Enter a whole number of days from 7 to 3650, or leave empty to keep files.";
    else if (nd !== undefined && daysMax !== null && nd > daysMax) err.days = `Your instance allows at most ${daysMax} days.`;
    if (nq === "invalid") err.quota = "Enter a whole number of MiB, or leave empty for the default.";
    else if (nq !== undefined && quotaMax !== null && (nq === 0 || nq > quotaMax)) err.quota = `Your instance allows at most ${quotaMax} MiB.`;
    setClientErr(err);
    if (err.days || err.quota) return;
    put.mutate(
      {
        attachments_days: typeof nd === "number" ? nd : null,
        attachment_quota_mib: typeof nq === "number" ? nq : null,
      },
      { onSuccess: () => setSaved(true) },
    );
  }

  const serverErr = put.error ? errorMessage(put.error) : undefined;
  return (
    <form onSubmit={submit} noValidate aria-label="Attachment limits" className="max-w-md space-y-3">
      <h2 className="text-lg font-medium">Attachment limits</h2>
      <Field
        id="attachments-days"
        label="Delete file contents after (days)"
        hint={`Files stay in transcripts as “expired by retention”. ${daysMax !== null ? `Between 7 and ${daysMax} days.` : "Between 7 and 3650 days."} Empty keeps them.`}
        error={clientErr.days ?? serverErr}
      >
        <input
          id="attachments-days"
          inputMode="numeric"
          className={inputClass}
          value={daysValue}
          onChange={(e) => {
            setDays(e.target.value);
            setSaved(false);
          }}
        />
      </Field>
      <Field
        id="attachment-quota"
        label="Storage quota (MiB)"
        hint={`Empty uses the default (${d.attachment_quota_mib_effective ?? "unlimited"} MiB in force).${quotaMax !== null ? ` At most ${quotaMax} MiB.` : " 0 means unlimited."}`}
        error={clientErr.quota}
      >
        <input
          id="attachment-quota"
          inputMode="numeric"
          className={inputClass}
          value={quotaValue}
          onChange={(e) => {
            setQuota(e.target.value);
            setSaved(false);
          }}
        />
      </Field>
      {saved ? (
        <p role="status" className="text-sm text-green-700">
          Limits saved.
        </p>
      ) : null}
      <Button type="submit" disabled={put.isPending}>
        {put.isPending ? "Saving…" : "Save limits"}
      </Button>
    </form>
  );
}

export function OrgStorageView({ orgId }: { orgId: string }) {
  const storage = useOrgStorage(orgId);
  if (storage.isPending) return <Loading />;
  if (storage.isError) return <ErrorState error={storage.error} onRetry={() => storage.refetch()} />;
  const s = storage.data;
  const notice = NOTICE[s.notice] ?? NOTICE.clear;
  return (
    <div className="max-w-4xl space-y-6">
      <PageHeader title="Storage" description="Attachment bytes used by this organization, by workspace and direction." />
      <Card className="space-y-3">
        <div className="flex items-center justify-between gap-3">
          <h2 className="text-lg font-medium">Usage</h2>
          <Badge tone={notice.tone}>{notice.label}</Badge>
        </div>
        <UsageBar used={s.used_bytes} quota={s.quota_bytes} />
        {s.notice === "full" ? (
          <p role="alert" className="text-sm text-danger-ink">
            Your organization&apos;s attachment storage is full. Uploads and files sent by agents are refused until space is freed or the
            quota is raised.
          </p>
        ) : null}
        <p className="text-sm text-ink-2">
          {s.files} files hold bytes, {s.purged_files} expired or deleted.
        </p>
      </Card>
      <Card>
        <h2 className="mb-2 text-lg font-medium">By workspace</h2>
        {s.by_workspace.length === 0 ? (
          <p className="text-sm text-ink-2">No attachments stored.</p>
        ) : (
          <div className="overflow-x-auto">
            <table className="w-full text-left text-sm">
              <thead className="text-muted">
                <tr>
                  <th className="py-2 pr-4 font-medium">Workspace</th>
                  <th className="py-2 pr-4 font-medium">Direction</th>
                  <th className="py-2 pr-4 text-right font-medium">Files</th>
                  <th className="py-2 text-right font-medium">Size</th>
                </tr>
              </thead>
              <tbody>
                {s.by_workspace.map((r) => (
                  <tr key={`${r.workspace_id}-${r.direction}`} className="border-t border-line">
                    <td className="py-2 pr-4">{r.workspace_name}</td>
                    <td className="py-2 pr-4">{r.direction === "in" ? "Uploaded by people" : "Sent by agents"}</td>
                    <td className="py-2 pr-4 text-right tabular-nums">{r.files}</td>
                    <td className="py-2 text-right tabular-nums">{formatBytes(r.bytes)}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
      </Card>
      <Card>
        <LimitsForm orgId={orgId} />
      </Card>
    </div>
  );
}
