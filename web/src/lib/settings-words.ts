import type { FieldInfo } from "@/lib/api/settings";

/** Seconds in each unit a duration is shown in. */
const UNIT_SECS: Record<string, number> = { seconds: 1, minutes: 60, hours: 3600, days: 86400 };

function plural(n: number, unit: string): string {
  const singular = unit.endsWith("s") ? unit.slice(0, -1) : unit;
  return n === 1 ? `1 ${singular}` : `${n} ${unit}`;
}

/** A number of seconds in the largest whole unit: "15 minutes", "1 hour", "45 seconds". */
export function durationWords(secs: number): string {
  for (const [size, unit] of [
    [86400, "days"],
    [3600, "hours"],
    [60, "minutes"],
  ] as const) {
    if (secs >= size && secs % size === 0) return plural(secs / size, unit);
  }
  return plural(secs, "seconds");
}

function scalarWords(field: FieldInfo | undefined, v: unknown): string {
  if (typeof v === "boolean") return v ? "On" : "Off";
  if (typeof v === "number") {
    if (field?.kind === "duration") return durationWords(v);
    return field?.unit && Number.isInteger(v) ? plural(v, field.unit) : String(v);
  }
  if (typeof v === "string") return field?.choices?.find((c) => c.value === v)?.label ?? v;
  if (typeof v === "object" && v !== null) {
    const o = v as Record<string, unknown>;
    const name = o.host ?? o.id ?? o.class;
    return typeof name === "string" ? name : "An entry";
  }
  return "";
}

/** A setting's value in words, never a raw enumeration id or JSON: "On", "Approved endpoints only", "None". */
export function valueWords(field: FieldInfo | undefined, v: unknown): string {
  if (v === null || v === undefined) return "Not set";
  if (Array.isArray(v)) {
    if (v.length === 0) return "None";
    if (field?.kind === "keyed_list") return `${v.length} configured`;
    return v.map((x) => scalarWords(field, x)).join(", ");
  }
  return scalarWords(field, v);
}

/** The unit's size in seconds (for a duration field), else 1. */
export function unitSecs(field: FieldInfo): number {
  return field.kind === "duration" ? (UNIT_SECS[field.unit ?? "seconds"] ?? 1) : 1;
}
