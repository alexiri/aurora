"use client";

import { useState, type FormEvent } from "react";

import { ErrorState, Loading } from "@/components/states";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { DataTable, Td, Th } from "@/components/ui/data-table";
import { Field, inputClass } from "@/components/ui/field";
import { Form, FormActions } from "@/components/ui/form";
import { Meter } from "@/components/ui/meter";
import { Notice } from "@/components/ui/notice";
import { PageHeader } from "@/components/ui/page-header";
import { Section } from "@/components/ui/section";
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
    <Form onSubmit={submit} noValidate aria-label="Attachment limits" className="max-w-md">
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
        <Notice tone="accent" role="status">
          Limits saved.
        </Notice>
      ) : null}
      <FormActions>
        <Button type="submit" size="lg" disabled={put.isPending}>
          {put.isPending ? "Saving…" : "Save limits"}
        </Button>
      </FormActions>
    </Form>
  );
}

export function OrgStorageView({ orgId }: { orgId: string }) {
  const storage = useOrgStorage(orgId);
  if (storage.isPending) return <Loading />;
  if (storage.isError) return <ErrorState error={storage.error} onRetry={() => storage.refetch()} />;
  const s = storage.data;
  const notice = NOTICE[s.notice] ?? NOTICE.clear;
  return (
    <div className="max-w-[1160px] space-y-8">
      <PageHeader title="Storage" description="Attachment bytes used by this organization, by workspace and direction." className="mb-6" />
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
      <Section id="attachment-limits" title="Attachment limits" card>
        <LimitsForm orgId={orgId} />
      </Section>
    </div>
  );
}
