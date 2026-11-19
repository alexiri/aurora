"use client";

import { Badge } from "@/components/ui/badge";

/** One tool of an app in a plain "pick some tools" list. */
export interface ToolChecklistItem {
  id: string;
  name: string;
  /** The package's default for the tool; "ask" shows the "asks first by default" marker. */
  approval?: string | null;
}

/**
 * (FIX-184 follow-up, D-233) The one plain "pick tools of this app" checklist. Lives with the permissions editor so no
 * other screen renders its own checkbox list of an app's tools. Where a tool needs an Allow / Ask / Block choice use
 * `PermissionGroups` instead; this is for naming a set (a rule's `app` target).
 */
export function ToolChecklist({
  items,
  picked,
  onChange,
  legend,
  loading = false,
}: {
  items: ToolChecklistItem[];
  picked: string[];
  onChange: (picked: string[]) => void;
  legend: string;
  loading?: boolean;
}) {
  return (
    <fieldset className="space-y-1 text-sm">
      <legend className="text-xs text-muted">{legend}</legend>
      {loading ? <p className="text-xs text-muted">Loading tools…</p> : null}
      {items.map((t) => (
        <label key={t.id} className="flex items-center gap-2">
          <input
            type="checkbox"
            checked={picked.includes(t.id)}
            onChange={(e) => onChange(e.target.checked ? [...picked, t.id] : picked.filter((x) => x !== t.id))}
          />
          <span>{t.name}</span>
          {t.approval === "ask" ? <Badge tone="accent">asks first by default</Badge> : null}
        </label>
      ))}
    </fieldset>
  );
}
