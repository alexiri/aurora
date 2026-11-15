"use client";

import Link from "next/link";
import { useState } from "react";

import { ImportDialog } from "@/components/admin/settings/import-dialog";
import { ErrorState, Loading } from "@/components/states";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Notice } from "@/components/ui/notice";
import { PageHeader } from "@/components/ui/page-header";
import { ItemList, ListRow } from "@/components/ui/section";
import { exportUrl, sectionSlug, useSettingsIndex } from "@/lib/api/settings";

const DISMISS_KEY = "aurora.settings.locked-notice.dismissed";

function readDismissed(): string | null {
  try {
    return window.localStorage.getItem(DISMISS_KEY);
  } catch {
    return null;
  }
}
function writeDismissed(v: string) {
  try {
    window.localStorage.setItem(DISMISS_KEY, v);
  } catch {
    // Private windows and blocked storage: the notice just comes back next time.
  }
}

function when(iso: string | null | undefined): string {
  if (!iso) return "never";
  return new Date(iso).toLocaleString();
}

/** Admin › Settings (`/admin/settings`, SET-1, `14 §Instance administration`): the sections, the locked keys, Export and Import. */
export function SettingsIndexView() {
  const q = useSettingsIndex();
  const [importing, setImporting] = useState(false);
  const [dismissed, setDismissed] = useState<string | null>(() => (typeof window === "undefined" ? null : readDismissed()));
  if (q.isPending) return <Loading />;
  if (q.isError) return <ErrorState error={q.error} onRetry={() => q.refetch()} />;
  const { sections, locked } = q.data;
  // Dismissed per viewer for this set of locked keys: a new locked key brings the notice back.
  const fingerprint = locked.map((l) => `${l.key}:${l.entry_id ?? ""}`).join("|");
  const showNotice = locked.length > 0 && dismissed !== fingerprint;

  return (
    <div className="max-w-page space-y-6">
      <PageHeader
        title="Settings"
        description="Instance-wide settings, applied live. A setting configured in Helm is shown read-only."
        actions={
          <div className="flex flex-wrap gap-2">
            <Button asChild variant="outline" size="sm">
              <a href={exportUrl("yaml")} download>
                Export YAML
              </a>
            </Button>
            <Button asChild variant="outline" size="sm">
              <a href={exportUrl("json")} download>
                Export JSON
              </a>
            </Button>
            <Button type="button" variant="outline" size="sm" onClick={() => setImporting(true)}>
              Import…
            </Button>
          </div>
        }
      />
      {showNotice ? (
        <Notice tone="neutral" role="status" className="space-y-2">
          <p>
            {locked.length} setting{locked.length === 1 ? " is" : "s are"} configured in Helm and cannot be changed here. To
            manage one here, remove it from your Helm values and upgrade: it keeps its current value.
          </p>
          <details>
            <summary className="cursor-pointer text-sm font-medium">Which settings</summary>
            <ul className="mt-2 space-y-1 text-sm">
              {locked.map((l) => (
                <li key={`${l.key}:${l.entry_id ?? ""}`}>
                  <code className="font-mono">{l.key}</code>
                  {l.entry_id ? <> ({l.entry_id})</> : null} — <code className="font-mono">{l.helm_key}</code>
                </li>
              ))}
            </ul>
          </details>
          <Button
            type="button"
            variant="ghost"
            size="sm"
            onClick={() => {
              writeDismissed(fingerprint);
              setDismissed(fingerprint);
            }}
          >
            Dismiss
          </Button>
        </Notice>
      ) : null}

      <ItemList aria-label="Settings sections">
        {sections.map((s) => (
          <ListRow
            key={s.section}
            actions={
              <Button asChild variant="outline" size="sm">
                <Link href={`/admin/settings/${sectionSlug(s.section)}`} aria-label={`Open ${s.title}`}>
                  Open
                </Link>
              </Button>
            }
          >
            <div className="min-w-0 space-y-1">
              <Link href={`/admin/settings/${sectionSlug(s.section)}`} className="font-medium text-ink hover:underline">
                {s.title}
              </Link>
              <p className="text-sm text-ink-2">{s.description}</p>
              <p className="text-xs text-muted">
                {s.keys_total} setting{s.keys_total === 1 ? "" : "s"}
                {s.keys_locked > 0 ? <> · <Badge tone="pending">{s.keys_locked} in Helm</Badge></> : null}
                {s.keys_dashboard > 0 ? <> · <Badge tone="accent">{s.keys_dashboard} in the dashboard</Badge></> : null}
                {" · "}Last change: {s.updated_by_label ? `${s.updated_by_label}, ` : ""}
                {when(s.updated_at)}
              </p>
            </div>
          </ListRow>
        ))}
      </ItemList>
      <ImportDialog open={importing} onOpenChange={setImporting} />
    </div>
  );
}
