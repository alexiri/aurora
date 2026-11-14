"use client";

import { PermissionGroups, type PermissionGroup } from "./permission-groups";

type OnOff = "on" | "off";

const OPTIONS = [
  { value: "on", label: "On" },
  { value: "off", label: "Off" },
] as const;

export interface ScopeItem {
  scope: string;
  name: string;
  /** `read` | `write`. */
  access: string;
}

/**
 * A provider's scopes in the shared permission pattern (FIX-147): grouped Read / Write, On / Off per scope and a
 * group-wide On / Off. Used by the connect window's "Requested scopes" and by the declared token scopes (FIX-136).
 */
export function ScopeGroups({
  scopes,
  selected,
  onChange,
  disabled = false,
  subject = "scopes",
}: {
  scopes: ScopeItem[];
  /** The scope ids that are on. */
  selected: string[];
  onChange: (next: string[]) => void;
  disabled?: boolean;
  /** Plural noun in the accessible names ("scopes"). */
  subject?: string;
}) {
  const on = new Set(selected);
  const values: Record<string, OnOff> = Object.fromEntries(scopes.map((s) => [s.scope, on.has(s.scope) ? "on" : "off"]));
  const groups: PermissionGroup[] = [
    { id: "read", label: "Read", rows: scopes.filter((s) => s.access !== "write") },
    { id: "write", label: "Write", rows: scopes.filter((s) => s.access === "write") },
  ].map((g) => ({
    id: g.id,
    label: g.label,
    rows: g.rows.map((s) => ({ id: s.scope, name: s.name, detail: s.scope })),
  }));

  const setMany = (ids: string[], v: OnOff) => {
    const next = new Set(selected);
    for (const id of ids) {
      if (v === "on") next.add(id);
      else next.delete(id);
    }
    // Keep the provider's own order so a request is stable.
    onChange(scopes.map((s) => s.scope).filter((id) => next.has(id)).concat([...next].filter((id) => !scopes.some((s) => s.scope === id))));
  };

  return (
    <PermissionGroups<OnOff>
      groups={groups}
      options={[...OPTIONS]}
      values={values}
      fallback="off"
      disabled={disabled}
      onChange={(id, v) => setMany([id], v)}
      onChangeGroup={(gid, v) => setMany(groups.find((g) => g.id === gid)?.rows.map((r) => r.id) ?? [], v)}
      sectionLabel={(g) => `${g.label} ${subject}`}
      groupControlLabel={(g) => `Set all ${g.label.toLowerCase()} ${subject}`}
      rowControlLabel={(r) => `${r.name}`}
      groupSummary={(g, v) => `${g.rows.filter((r) => v[r.id] === "on").length} of ${g.rows.length} on`}
    />
  );
}
