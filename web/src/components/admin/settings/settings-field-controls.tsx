"use client";

import { useState, type ReactNode } from "react";

import { Badge } from "@/components/ui/badge";
import { LockPill } from "@/components/ui/lock-pill";
import { Button } from "@/components/ui/button";
import { inputClass } from "@/components/ui/field";
import { CheckboxField } from "@/components/ui/form";
import type { CurrentValue, FieldInfo } from "@/lib/api/settings";
import { unitSecs } from "@/lib/settings-words";

/** A number with its unit beside it. A duration stays in seconds on the wire and is shown in `unit`. */
export function UnitNumberControl({
  id,
  field,
  value,
  min,
  max,
  decimal,
  onChange,
  onClear,
}: {
  id: string;
  field: FieldInfo;
  value: unknown;
  /** The registry's bounds, in the stored unit (seconds for a duration). */
  min?: number | null;
  max?: number | null;
  decimal?: boolean;
  onChange: (v: number) => void;
  onClear: () => void;
}) {
  const size = unitSecs(field);
  const shown = typeof value === "number" ? value / size : null;
  return (
    <div className="flex flex-wrap items-center gap-2">
      <input
        id={id}
        type="number"
        inputMode={decimal ? "decimal" : "numeric"}
        step={decimal ? "any" : undefined}
        className={`${inputClass} max-w-40`}
        min={decimal || min === null || min === undefined ? undefined : Math.ceil(min / size)}
        max={decimal || max === null || max === undefined ? undefined : Math.floor(max / size)}
        value={shown === null ? "" : String(Math.round(shown * 1000) / 1000)}
        onChange={(e) => (e.target.value === "" ? onClear() : onChange(Math.round(Number(e.target.value) * size * 1000) / 1000))}
      />
      {field.unit ? <span className="text-sm text-ink-2">{field.unit}</span> : null}
    </div>
  );
}

/**
 * The rows an editor shows, which may include a blank one being typed. `emit` turns them into the value the form
 * keeps; when the form's value changes by anything else (Reset to default, Discard changes) the rows follow it.
 */
function useRows<T>(value: unknown, emit: (rows: T[]) => T[], onChange: (v: T[]) => void) {
  const incoming = JSON.stringify(Array.isArray(value) ? value : []);
  const [seen, setSeen] = useState(incoming);
  const [rows, setRows] = useState<T[]>(() => (Array.isArray(value) ? (value as T[]) : []));
  if (incoming !== seen) {
    setSeen(incoming);
    setRows(Array.isArray(value) ? (value as T[]) : []);
  }
  const commit = (next: T[]) => {
    const out = emit(next);
    setRows(next);
    setSeen(JSON.stringify(out));
    onChange(out);
  };
  return { rows, setRows, commit };
}

/** A list of text entries, one input per entry with Add and Remove — never JSON, never a text blob. */
export function StringListEditor({
  id,
  label,
  value,
  onChange,
}: {
  id: string;
  label: string;
  value: unknown;
  onChange: (v: string[]) => void;
}) {
  const { rows, setRows, commit } = useRows<string>(value, (next) => next.map((x) => x.trim()).filter(Boolean), onChange);
  return (
    <div id={id} className="space-y-2">
      {rows.length === 0 ? <p className="text-sm text-ink-2">None yet.</p> : null}
      <ul aria-label={label} className="space-y-2">
        {rows.map((r, i) => (
          <li key={i} className="flex items-center gap-2">
            <input
              aria-label={`${label}: entry ${i + 1}`}
              className={`${inputClass} font-mono`}
              value={r}
              onChange={(e) => commit(rows.map((x, j) => (j === i ? e.target.value : x)))}
            />
            <Button type="button" variant="ghost" size="sm" aria-label={`Remove ${r || `entry ${i + 1}`}`} onClick={() => commit(rows.filter((_, j) => j !== i))}>
              Remove
            </Button>
          </li>
        ))}
      </ul>
      <Button type="button" variant="outline" size="sm" onClick={() => setRows([...rows, ""])}>
        Add
      </Button>
    </div>
  );
}

/** A list whose entries come from a fixed set of choices: a checkbox each, with its sentence. */
export function ChoiceListEditor({
  id,
  field,
  value,
  onChange,
}: {
  id: string;
  field: FieldInfo;
  value: unknown;
  onChange: (v: string[]) => void;
}) {
  const chosen = new Set(Array.isArray(value) ? (value as string[]) : []);
  return (
    <div id={id} role="group" aria-label={field.label} className="space-y-2">
      {(field.choices ?? []).map((c) => (
        <CheckboxField
          key={c.value}
          id={`${id}-${c.value}`}
          label={c.label}
          hint={c.description}
          checked={chosen.has(c.value)}
          onChange={(on) => {
            const next = new Set(chosen);
            if (on) next.add(c.value);
            else next.delete(c.value);
            const known = (field.choices ?? []).map((x) => x.value);
            // A stored value that is not a choice is kept, after the chosen ones.
            onChange([...known.filter((v) => next.has(v)), ...[...next].filter((v) => !known.includes(v))]);
          }}
        />
      ))}
    </div>
  );
}

interface HostEntry {
  host: string;
  path?: string;
}

/** The always-allowed websites: a host name per row, with an optional path. */
export function HostListEditor({
  id,
  label,
  value,
  onChange,
}: {
  id: string;
  label: string;
  value: unknown;
  onChange: (v: HostEntry[]) => void;
}) {
  const { rows, setRows, commit } = useRows<HostEntry>(
    value,
    (next) =>
      next
        .filter((r) => r.host.trim() !== "")
        .map((r) => (r.path?.trim() ? { host: r.host.trim(), path: r.path.trim() } : { host: r.host.trim() })),
    onChange,
  );
  return (
    <div id={id} className="space-y-2">
      {rows.length === 0 ? <p className="text-sm text-ink-2">None yet.</p> : null}
      <ul aria-label={label} className="space-y-2">
        {rows.map((r, i) => (
          <li key={i} className="flex flex-wrap items-center gap-2">
            <input
              aria-label={`${label}: website ${i + 1}`}
              placeholder="Website, for example api.example.com"
              className={`${inputClass} max-w-xs`}
              value={r.host}
              onChange={(e) => commit(rows.map((x, j) => (j === i ? { ...x, host: e.target.value } : x)))}
            />
            <input
              aria-label={`${label}: path ${i + 1}`}
              placeholder="Path (optional)"
              className={`${inputClass} max-w-48`}
              value={r.path ?? ""}
              onChange={(e) => commit(rows.map((x, j) => (j === i ? { ...x, path: e.target.value } : x)))}
            />
            <Button type="button" variant="ghost" size="sm" aria-label={`Remove ${r.host || `website ${i + 1}`}`} onClick={() => commit(rows.filter((_, j) => j !== i))}>
              Remove
            </Button>
          </li>
        ))}
      </ul>
      <Button type="button" variant="outline" size="sm" onClick={() => setRows([...rows, { host: "" }])}>
        Add
      </Button>
    </div>
  );
}

/**
 * A section whose keys are not editable yet (C-973): each current value in words with a source chip and where to
 * change it. No form, no Save.
 */
export function CurrentValues({ title, current, fields }: { title: string; current: CurrentValue[]; fields: FieldInfo[] }): ReactNode {
  const byKey = new Map(fields.map((f) => [f.key, f]));
  const docTitles = [...new Set(fields.map((f) => f.doc?.title).filter((t): t is string => Boolean(t)))];
  return (
    <section aria-label={title} className="space-y-3 rounded-card border border-line bg-surface p-4 sm:p-5">
      <dl className="space-y-3">
        {current.map((c) => {
          const f = byKey.get(c.key);
          return (
            <div key={c.key} id={c.key} className="min-w-0 scroll-mt-24 space-y-1 border-b border-line pb-3 last:border-b-0 last:pb-0">
              <dt className="flex flex-wrap items-center gap-x-2 gap-y-1 text-sm font-medium text-ink">
                {f?.label ?? c.label}
                {c.source === "helm" ? <LockPill owner="Set in Helm" /> : <Badge>Default</Badge>}
              </dt>
              <dd className="break-words text-sm text-ink" data-testid={`${c.key}-current`}>
                {c.value_display}
              </dd>
              {f ? <dd className="text-xs text-muted">{f.help}</dd> : null}
            </div>
          );
        })}
      </dl>
      <p className="text-sm text-ink-2">
        You cannot change these here yet. Change them in your Helm values
        {docTitles.length > 0 ? <>: see “{docTitles.join("”, “")}” in the operator guide</> : null}.
      </p>
    </section>
  );
}
