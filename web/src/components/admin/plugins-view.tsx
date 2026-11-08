"use client";

import Link from "next/link";
import { useState } from "react";

import { fmtDate } from "@/components/account/key-table";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { DataTable, Td, Th } from "@/components/ui/data-table";
import { Dialog, DialogContent, DialogTrigger } from "@/components/ui/dialog";
import { inputClass } from "@/components/ui/field";
import { Notice } from "@/components/ui/notice";
import { PageHeader } from "@/components/ui/page-header";
import { EmptyState, ErrorState, Loading } from "@/components/states";
import { ApiError } from "@/lib/api/client";
import { errorMessage } from "@/lib/api/errors";
import { usePlugins, useUploadPlugin, type PluginViolation } from "@/lib/api/plugins";

const STATUSES = ["", "pending_approval", "active", "disabled", "rejected", "superseded", "invalid"];

export function violationsOf(err: unknown): PluginViolation[] {
  if (!(err instanceof ApiError)) return [];
  const d = err.details;
  if (typeof d !== "object" || d === null || !("violations" in d)) return [];
  const v = (d as { violations: unknown }).violations;
  return Array.isArray(v) ? (v as PluginViolation[]) : [];
}

export function ViolationList({ violations }: { violations: PluginViolation[] }) {
  if (violations.length === 0) return null;
  return (
    <ul aria-label="Violations" className="space-y-1 text-xs text-danger-ink">
      {violations.map((v, i) => (
        <li key={`${v.code}-${v.path}-${i}`}>
          <code className="font-mono">{v.code}</code> {v.path ? <code className="font-mono">{v.path}</code> : null} {v.message}
        </li>
      ))}
    </ul>
  );
}

/** Tone for a plugin lifecycle status (text always shown beside it). */
export function statusTone(status: string): "neutral" | "accent" | "pending" | "danger" {
  if (status === "active") return "accent";
  if (status === "pending_approval") return "pending";
  if (status === "rejected" || status === "invalid") return "danger";
  return "neutral";
}

export function UploadForm({ onDone }: { onDone?: () => void }) {
  const upload = useUploadPlugin();
  const [file, setFile] = useState<File | null>(null);
  return (
    <form
      className="space-y-4"
      onSubmit={(e) => {
        e.preventDefault();
        if (!file) return;
        upload.mutate(file, { onSuccess: () => onDone?.() });
      }}
    >
      <div className="space-y-1.5">
        <label className="block text-sm font-medium text-ink" htmlFor="plugin-package">
          Plugin package
        </label>
        <input
          id="plugin-package"
          type="file"
          className={`${inputClass} file:mr-3 file:rounded-control file:border-0 file:bg-subtle file:px-3 file:py-1 file:text-sm file:font-medium file:text-ink`}
          onChange={(e) => setFile(e.target.files?.[0] ?? null)}
        />
      </div>
      {upload.isError ? (
        <Notice tone="danger" role="alert" className="space-y-1">
          <p>{errorMessage(upload.error)}</p>
          <ViolationList violations={violationsOf(upload.error)} />
        </Notice>
      ) : null}
      {upload.data?.status === "invalid" ? (
        <Notice tone="danger" role="alert" className="space-y-1">
          <p>The package was stored but is invalid.</p>
          <ViolationList violations={upload.data.violations} />
        </Notice>
      ) : null}
      <div className="flex justify-end">
        <Button type="submit" size="lg" disabled={!file || upload.isPending}>
          {upload.isPending ? "Uploading…" : "Upload"}
        </Button>
      </div>
    </form>
  );
}

export function PluginsView() {
  const [status, setStatus] = useState("");
  const [open, setOpen] = useState(false);
  const q = usePlugins(status);
  return (
    <div className="max-w-[1160px] space-y-4">
      <PageHeader
        title="Plugins"
        description="Signed packages that add apps and connectors to this instance."
        className="mb-2"
        actions={
          <Dialog open={open} onOpenChange={setOpen}>
            <DialogTrigger asChild>
              <Button>Upload plugin</Button>
            </DialogTrigger>
            <DialogContent title="Upload plugin" description="Upload a signed plugin package for review.">
              <UploadForm onDone={() => setOpen(false)} />
            </DialogContent>
          </Dialog>
        }
      />
      <div className="max-w-xs space-y-1.5">
        <label htmlFor="plugin-status" className="block text-sm font-medium text-ink">
          Status
        </label>
        <select id="plugin-status" className={inputClass} value={status} onChange={(e) => setStatus(e.target.value)}>
          {STATUSES.map((s) => (
            <option key={s} value={s}>
              {s ? s.replace("_", " ") : "All"}
            </option>
          ))}
        </select>
      </div>
      {q.isPending ? <Loading /> : null}
      {q.isError ? <ErrorState error={q.error} onRetry={() => q.refetch()} /> : null}
      {q.data && q.data.items.length === 0 ? <EmptyState title="No plugins" /> : null}
      {q.data && q.data.items.length > 0 ? (
        <DataTable caption="Plugins">
          <thead>
            <tr>
              <Th>Package</Th>
              <Th>Version</Th>
              <Th>Signer</Th>
              <Th>Status</Th>
              <Th>Added</Th>
            </tr>
          </thead>
          <tbody>
            {q.data.items.map((p) => (
              <tr key={p.id}>
                <Td>
                  <Link href={`/admin/plugins/${p.id}`} className="font-medium text-link underline">
                    {p.package_id}
                  </Link>
                  <div className="text-muted">{p.publisher}</div>
                </Td>
                <Td className="font-mono text-xs">{p.version}</Td>
                <Td>{p.signer.kind === "trusted_key" ? (p.signer.name ?? "trusted key") : p.signer.kind}</Td>
                <Td>
                  <Badge tone={statusTone(p.status)}>{p.status.replace("_", " ")}</Badge>
                </Td>
                <Td className="whitespace-nowrap text-ink-2">{fmtDate(p.created_at)}</Td>
              </tr>
            ))}
          </tbody>
        </DataTable>
      ) : null}
    </div>
  );
}
