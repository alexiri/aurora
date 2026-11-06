"use client";

import Link from "next/link";
import { useState } from "react";

import { fmtDate } from "@/components/account/key-table";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogTrigger } from "@/components/ui/dialog";
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
    <ul aria-label="Violations" className="space-y-1 text-xs text-red-800">
      {violations.map((v, i) => (
        <li key={`${v.code}-${v.path}-${i}`}>
          <code>{v.code}</code> {v.path ? <code>{v.path}</code> : null} {v.message}
        </li>
      ))}
    </ul>
  );
}

export function UploadForm({ onDone }: { onDone?: () => void }) {
  const upload = useUploadPlugin();
  const [file, setFile] = useState<File | null>(null);
  return (
    <form
      className="space-y-3"
      onSubmit={(e) => {
        e.preventDefault();
        if (!file) return;
        upload.mutate(file, { onSuccess: () => onDone?.() });
      }}
    >
      <label className="block text-sm font-medium" htmlFor="plugin-package">
        Plugin package
      </label>
      <input
        id="plugin-package"
        type="file"
        onChange={(e) => setFile(e.target.files?.[0] ?? null)}
      />
      {upload.isError ? (
        <div role="alert" className="space-y-1 text-sm text-red-700">
          <p>{errorMessage(upload.error)}</p>
          <ViolationList violations={violationsOf(upload.error)} />
        </div>
      ) : null}
      {upload.data?.status === "invalid" ? (
        <div role="alert" className="space-y-1 text-sm text-red-700">
          <p>The package was stored but is invalid.</p>
          <ViolationList violations={upload.data.violations} />
        </div>
      ) : null}
      <Button type="submit" disabled={!file || upload.isPending}>
        {upload.isPending ? "Uploading…" : "Upload"}
      </Button>
    </form>
  );
}

export function PluginsView() {
  const [status, setStatus] = useState("");
  const [open, setOpen] = useState(false);
  const q = usePlugins(status);
  return (
    <div className="space-y-4">
      <div className="flex items-center justify-between">
        <h1 className="text-xl font-semibold">Plugins</h1>
        <Dialog open={open} onOpenChange={setOpen}>
          <DialogTrigger asChild>
            <Button>Upload plugin</Button>
          </DialogTrigger>
          <DialogContent title="Upload plugin" description="Upload a signed plugin package for review.">
            <UploadForm onDone={() => setOpen(false)} />
          </DialogContent>
        </Dialog>
      </div>
      <div className="max-w-xs">
        <label htmlFor="plugin-status" className="block text-sm font-medium">
          Status
        </label>
        <select
          id="plugin-status"
          className="block w-full rounded-md border border-neutral-300 bg-white px-3 py-2 text-sm"
          value={status}
          onChange={(e) => setStatus(e.target.value)}
        >
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
        <table className="w-full text-left text-sm">
          <thead className="text-neutral-500">
            <tr>
              <th className="py-2 pr-4 font-medium">Package</th>
              <th className="py-2 pr-4 font-medium">Version</th>
              <th className="py-2 pr-4 font-medium">Signer</th>
              <th className="py-2 pr-4 font-medium">Status</th>
              <th className="py-2 font-medium">Added</th>
            </tr>
          </thead>
          <tbody>
            {q.data.items.map((p) => (
              <tr key={p.id} className="border-t border-neutral-200">
                <td className="py-2 pr-4">
                  <Link href={`/admin/plugins/${p.id}`} className="font-medium underline">
                    {p.package_id}
                  </Link>
                  <div className="text-neutral-500">{p.publisher}</div>
                </td>
                <td className="py-2 pr-4">{p.version}</td>
                <td className="py-2 pr-4">{p.signer.kind === "trusted_key" ? (p.signer.name ?? "trusted key") : p.signer.kind}</td>
                <td className="py-2 pr-4">{p.status.replace("_", " ")}</td>
                <td className="py-2">{fmtDate(p.created_at)}</td>
              </tr>
            ))}
          </tbody>
        </table>
      ) : null}
    </div>
  );
}
