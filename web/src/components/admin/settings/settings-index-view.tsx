"use client";

import Link from "next/link";

import { formatExact } from "@/lib/format-date";
import { useState } from "react";

import { ImportDialog } from "@/components/admin/settings/import-dialog";
import { ErrorState, Loading } from "@/components/states";
import { Badge } from "@/components/ui/badge";
import { Altitude } from "@/components/ui/altitude";
import { Button } from "@/components/ui/button";
import { inputClass } from "@/components/ui/field";
import { Notice } from "@/components/ui/notice";
import { PageHeader } from "@/components/ui/page-header";
import { ItemList, ListRow } from "@/components/ui/section";
import { exportUrl, useSettingsIndex } from "@/lib/api/settings";
import { adminRoutes, sectionHomes } from "@/lib/routes";
import { personName } from "@/lib/workspace-name";

function when(iso: string | null | undefined): string {
  if (!iso) return "never";
  return formatExact(iso);
}

type SectionRow = NonNullable<ReturnType<typeof useSettingsIndex>["data"]>["sections"][number];

/** The order the topics appear in the sidebar (`14` topics per level); sections without a topic come last. */
const TOPIC_ORDER = ["What agents may do", "Apps", "AI endpoints", "People & sign-in", "Data", "Health & runners", "Other"];

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
  const [search, setSearch] = useState("");
  if (q.isPending) return <Loading />;
  if (q.isError) return <ErrorState error={q.error} onRetry={() => q.refetch()} />;
  const { sections, locked } = q.data;
  const groups = groupSections(sections, search);
  const showNotice = locked.length > 0;

  return (
    <div className="max-w-page space-y-6">
      <PageHeader
        title="All settings"
        description="Every instance-wide setting, grouped by the topic where you change it, applied live. A setting configured in Helm is shown read-only."
        actions={
          <Altitude id="export.yaml" level="expert">
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
          </Altitude>
        }
      />
      {showNotice ? (
        <Notice tone="neutral" role="status">
          <p>
            {locked.length} setting{locked.length === 1 ? " is" : "s are"} set in Helm.{" "}
            <Link href={adminRoutes.setInHelm()} className="font-medium underline underline-offset-2">
              See them on Set in Helm
            </Link>
          </p>
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
                      {" · "}Last change: {s.updated_by ? `${personName(s.updated_by.display_name)}, ` : ""}
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
