"use client";

import type { ReactNode } from "react";

import { Segmented, type SegmentedOption } from "@/components/ui/segmented";

/** One row of a permission group: a tool, a scope. */
export interface PermissionRow {
  id: string;
  name: string;
  /** Muted second line (the request a tool covers, what a scope allows). */
  detail?: string;
  /** Next to the name (an "Umbrella" badge). */
  badges?: ReactNode;
  /** A problem with this row (a 422 naming it). */
  error?: string;
}

export interface PermissionGroup {
  id: string;
  label: string;
  rows: PermissionRow[];
}

/**
 * The one grouped permission list of the dashboard (FIX-147, D-233): rows grouped Read / Write, a group-wide control
 * and a control per row. The per-agent grant editor uses it with Allow / Ask / Block, the connect window's scopes and
 * the declared token scopes with On / Off. The controls are `Segmented` (radio inputs), so arrow keys, Tab order and
 * accessible names are the same everywhere. A group whose rows differ shows no option checked and says "Mixed".
 */
export function PermissionGroups<V extends string>({
  groups,
  options,
  values,
  fallback,
  onChange,
  onChangeGroup,
  sectionLabel,
  groupControlLabel,
  rowControlLabel,
  groupSummary,
  disabled = false,
}: {
  groups: PermissionGroup[];
  options: SegmentedOption<V>[];
  values: Record<string, V>;
  /** The value of a row that has none yet. */
  fallback: V;
  onChange: (rowId: string, value: V) => void;
  onChangeGroup: (groupId: string, value: V) => void;
  /** Accessible name of a group's region, e.g. "Read tools". */
  sectionLabel: (group: PermissionGroup) => string;
  /** Accessible name of the group-wide control, e.g. "Set all read tools". */
  groupControlLabel: (group: PermissionGroup) => string;
  /** Accessible name of a row's control, e.g. "Access for Read an event". */
  rowControlLabel: (row: PermissionRow) => string;
  /** Short count shown next to the group heading, e.g. "2 of 5 on". */
  groupSummary?: (group: PermissionGroup, values: Record<string, V>) => string;
  disabled?: boolean;
}) {
  return (
    <>
      {groups.map((g) => {
        if (g.rows.length === 0) return null;
        const first = values[g.rows[0].id] ?? fallback;
        const uniform = g.rows.every((r) => (values[r.id] ?? fallback) === first);
        return (
          <section key={g.id} aria-label={sectionLabel(g)} className="space-y-1.5">
            <div className="flex flex-wrap items-center justify-between gap-x-3 gap-y-1.5">
              <h3 className="text-sm font-semibold text-ink">
                {g.label}
                {groupSummary ? <span className="ml-2 text-xs font-normal text-muted">{groupSummary(g, values)}</span> : null}
                {!uniform ? <span className="sr-only"> (mixed)</span> : null}
              </h3>
              <Segmented<V>
                label={groupControlLabel(g)}
                value={uniform ? first : null}
                options={options}
                onChange={(v) => onChangeGroup(g.id, v)}
                disabled={disabled}
              />
            </div>
            <ul className="divide-y divide-line rounded-control border border-line">
              {g.rows.map((r) => (
                <li key={r.id} className="space-y-1.5 px-3 py-2">
                  <div className="flex flex-wrap items-center gap-x-2 gap-y-1">
                    <span className="min-w-0 break-words text-sm font-medium text-ink">{r.name}</span>
                    {r.badges}
                  </div>
                  {r.detail ? <p className="break-all text-xs text-muted">{r.detail}</p> : null}
                  {r.error ? (
                    <p role="alert" className="text-xs text-danger-ink">
                      {r.error}
                    </p>
                  ) : null}
                  <Segmented<V>
                    label={rowControlLabel(r)}
                    value={values[r.id] ?? fallback}
                    options={options}
                    onChange={(v) => onChange(r.id, v)}
                    disabled={disabled}
                  />
                </li>
              ))}
            </ul>
          </section>
        );
      })}
    </>
  );
}
