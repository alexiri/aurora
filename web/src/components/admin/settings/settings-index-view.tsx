"use client";

import Link from "next/link";
import { useState } from "react";

import { ImportDialog } from "@/components/admin/settings/import-dialog";
import { ErrorState, Loading } from "@/components/states";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { inputClass } from "@/components/ui/field";
import { Notice } from "@/components/ui/notice";
import { PageHeader } from "@/components/ui/page-header";
import { ItemList, ListRow } from "@/components/ui/section";
import { exportUrl, useSettingsIndex } from "@/lib/api/settings";
import { sectionHomes } from "@/lib/routes";

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

type SectionRow = NonNullable<ReturnType<typeof useSettingsIndex>["data"]>["sections"][number];

/** The order the topics appear in the sidebar (`14` topics per level); sections without a topic come last. */
const TOPIC_ORDER = ["Agents", "Apps & accounts", "AI models", "Internet access", "People", "Data", "Platform", "Other"];

/** The fields of a section whose label or help contains `search` (case-insensitive; C-972). */
export function matchingFields(s: SectionRow, search: string): SectionRow["fields"] {
  const needle = search.trim().toLowerCase();
  if (!needle) return [];
  return (s.fields ?? []).filter((f) => `${f.label} ${f.help}`.toLowerCase().includes(needle));
}

/**
 * Sections grouped by topic, in sidebar order, those matching `search`: the section's name, description or topic, or
 * the label or help of any of its fields (case-insensitive).
 */
export function groupSections(sections: SectionRow[], search: string): { topic: string; sections: SectionRow[] }[] {
  const needle = search.trim().toLowerCase();
  const byTopic = new Map<string, SectionRow[]>();
  for (const s of sections) {
    const homes = sectionHomes(s.section);
    const hay = `${s.title} ${s.description} ${homes.map((h) => h.label).join(" ")}`.toLowerCase();
    if (needle && !hay.includes(needle) && matchingFields(s, search).length === 0) continue;
    const topic = homes[0]!.topic;
    byTopic.set(topic, [...(byTopic.get(topic) ?? []), s]);
  }
  return [...byTopic.entries()]
    .sort(([a], [b]) => (TOPIC_ORDER.indexOf(a) + 1 || 99) - (TOPIC_ORDER.indexOf(b) + 1 || 99))
    .map(([topic, list]) => ({ topic, sections: list }));
}

/** All settings (`/admin/settings`, SET-1, C-971): every section under its topic, with search, the locked keys, Export and Import. */
export function SettingsIndexView() {
  const q = useSettingsIndex();
  const [importing, setImporting] = useState(false);
  const [dismissed, setDismissed] = useState<string | null>(() => (typeof window === "undefined" ? null : readDismissed()));
  const [search, setSearch] = useState("");
  if (q.isPending) return <Loading />;
  if (q.isError) return <ErrorState error={q.error} onRetry={() => q.refetch()} />;
  const { sections, locked } = q.data;
  const groups = groupSections(sections, search);
  // Dismissed per viewer for this set of locked keys: a new locked key brings the notice back.
  const fingerprint = locked.map((l) => `${l.key}:${l.entry_id ?? ""}`).join("|");
  const showNotice = locked.length > 0 && dismissed !== fingerprint;

  return (
    <div className="max-w-page space-y-6">
      <PageHeader
        title="All settings"
        description="Every instance-wide setting, grouped by the topic where you change it, applied live. A setting configured in Helm is shown read-only."
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

      <div className="max-w-md">
        <label htmlFor="settings-search" className="sr-only">
          Search settings
        </label>
        <input
          id="settings-search"
          type="search"
          className={inputClass}
          placeholder="Search settings by name or what they do"
          value={search}
          onChange={(e) => setSearch(e.target.value)}
        />
      </div>
      {groups.length === 0 ? (
        <p className="text-sm text-ink-2" role="status">
          No settings match “{search.trim()}”.
        </p>
      ) : null}
      {groups.map((g) => (
        <section key={g.topic} aria-labelledby={`settings-topic-${g.topic}`} className="space-y-2">
          <h2 id={`settings-topic-${g.topic}`} className="text-base font-semibold text-ink">
            {g.topic}
          </h2>
          <ItemList aria-label={`${g.topic} settings`}>
            {g.sections.map((s) => {
              const homes = sectionHomes(s.section);
              return (
                <ListRow
                  key={s.section}
                  actions={
                    <Button asChild variant="outline" size="sm">
                      <Link href={homes[0]!.href} aria-label={`Open ${s.title}`}>
                        Open
                      </Link>
                    </Button>
                  }
                >
                  <div className="min-w-0 space-y-1">
                    <Link href={homes[0]!.href} className="font-medium text-ink hover:underline">
                      {s.title}
                    </Link>
                    <p className="text-sm text-ink-2">{s.description}</p>
                    {matchingFields(s, search).length > 0 ? (
                      <ul aria-label={`Settings in ${s.title} that match`} className="space-y-1 text-sm">
                        {matchingFields(s, search).map((f) => (
                          <li key={f.key}>
                            <Link href={`${homes[0]!.href}#${f.key}`} className="font-medium text-ink underline-offset-2 hover:underline">
                              {f.label}
                            </Link>
                            <span className="text-ink-2"> — {f.help}</span>
                          </li>
                        ))}
                      </ul>
                    ) : null}
                    {homes.length > 1 ? (
                      <p className="text-xs text-ink-2">
                        Also on:{" "}
                        {homes.slice(1).map((h, i) => (
                          <span key={h.href}>
                            {i > 0 ? ", " : ""}
                            <Link href={h.href} className="underline">
                              {h.label}
                            </Link>
                          </span>
                        ))}
                      </p>
                    ) : null}
                    <p className="text-xs text-muted">
                      {s.keys_total} setting{s.keys_total === 1 ? "" : "s"}
                      {s.keys_locked > 0 ? <> · <Badge>{s.keys_locked} in Helm</Badge></> : null}
                      {s.keys_dashboard > 0 ? <> · <Badge tone="accent">{s.keys_dashboard} in the dashboard</Badge></> : null}
                      {" · "}Last change: {s.updated_by_label ? `${s.updated_by_label}, ` : ""}
                      {when(s.updated_at)}
                    </p>
                  </div>
                </ListRow>
              );
            })}
          </ItemList>
        </section>
      ))}
      <ImportDialog open={importing} onOpenChange={setImporting} />
    </div>
  );
}
