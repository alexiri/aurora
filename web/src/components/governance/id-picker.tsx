"use client";

import { Button } from "@/components/ui/button";

export interface PickOption {
  id: string;
  label: string;
}

/** The ids of a governance textarea value ("one per line / comma separated"). */
export function splitIds(v: string): string[] {
  return v
    .split(/[\n,]+/)
    .map((s) => s.trim())
    .filter(Boolean);
}
export const joinIds = (ids: string[]): string => ids.join("\n");

/**
 * (P5-REFINE-17, org-7) Origin lists as names: the chosen organizations, workspaces or teams as removable chips and a
 * pick list of the others. The stored value stays the id list; an id the lists do not know shows as "Unknown (…)" with
 * its short id so it can still be removed. Nobody types an id.
 */
export function IdPicker({
  id,
  label,
  value,
  options,
  readOnly,
  onChange,
  empty = "No limit",
}: {
  id: string;
  label: string;
  value: string;
  options: PickOption[];
  readOnly?: boolean;
  onChange: (v: string) => void;
  empty?: string;
}) {
  const ids = splitIds(value);
  const name = (x: string) => options.find((o) => o.id === x)?.label ?? `Unknown (${x.slice(0, 8)})`;
  const rest = options.filter((o) => !ids.includes(o.id));
  return (
    <div className="space-y-1.5 text-sm">
      <label htmlFor={id} className="font-medium text-ink">
        {label}
      </label>
      <ul aria-label={label} className="flex flex-wrap gap-1.5">
        {ids.length === 0 ? <li className="text-ink-2">{empty}</li> : null}
        {ids.map((x) => (
          <li key={x} className="inline-flex items-center gap-1 rounded-full bg-subtle px-2.5 py-0.5 text-ink">
            {name(x)}
            {readOnly ? null : (
              <Button type="button" variant="ghost" size="sm" className="min-h-6 px-1" aria-label={`Remove ${name(x)}`} onClick={() => onChange(joinIds(ids.filter((y) => y !== x)))}>
                ×
              </Button>
            )}
          </li>
        ))}
      </ul>
      {readOnly || rest.length === 0 ? null : (
        <select
          id={id}
          className="h-9 rounded-control border border-line bg-surface px-2 text-sm text-ink"
          value=""
          onChange={(e) => e.target.value && onChange(joinIds([...ids, e.target.value]))}
        >
          <option value="">Add…</option>
          {rest.map((o) => (
            <option key={o.id} value={o.id}>
              {o.label}
            </option>
          ))}
        </select>
      )}
    </div>
  );
}
