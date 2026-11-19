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
    if (field?.unit === "bytes") return bytesWords(v);
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

const GB = 1024 ** 3;
const MB = 1024 ** 2;

/** A byte count in the largest whole-ish unit: "5 GB", "512 MB", "100 bytes". */
export function bytesWords(n: number): string {
  const round = (x: number) => String(Math.round(x * 10) / 10);
  if (n >= GB) return `${round(n / GB)} GB`;
  if (n >= MB) return `${round(n / MB)} MB`;
  return plural(n, "bytes");
}

/** "5368709120 bytes" (the server's default in words) → "5 GB"; any other text is unchanged. */
export function defaultInWords(text: string): string {
  const m = /^(\d+) bytes$/.exec(text.trim());
  return m ? bytesWords(Number(m[1])) : text;
}

/**
 * The unit a number control edits in and the size of that unit in the stored unit: a duration in its `unit`, a byte
 * count in GB or MB by the size of its default (stable while typing), anything else as it is.
 */
export function editUnit(field: FieldInfo): { size: number; label: string | null } {
  if (field.unit === "bytes") {
    const n = Number(/^(\d+) bytes$/.exec(field.default_display.trim())?.[1] ?? 0);
    if (n >= GB) return { size: GB, label: "GB" };
    if (n >= MB) return { size: MB, label: "MB" };
    return { size: 1, label: "bytes" };
  }
  return { size: unitSecs(field), label: field.unit ?? null };
}
