/** Number, duration and range helpers of the usage views (`14 §Usage`, P5-USAGE-2). */

/** `1,234,567` — the exact value, for tooltips and the table view. */
export const exact = (v: number): string => v.toLocaleString("en-US");

/** Counts compact for tiles and axes: `950`, `34 k`, `1.2 M`, `3.4 G`; the exact value goes in the tooltip. */
export function compact(v: number): string {
  const a = Math.abs(v);
  if (a < 1000) return String(Math.round(v));
  const unit = (div: number, s: string) => {
    const x = v / div;
    // one decimal below 10, none from 10 up: "1.2 M", "34 k"
    return `${Math.abs(x) < 10 ? x.toFixed(1).replace(/\.0$/, "") : Math.round(x)} ${s}`;
  };
  if (a < 999_500) return unit(1000, "k");
  if (a < 1_000_000_000) return unit(1_000_000, "M");
  return unit(1_000_000_000, "G");
}

/** "3 h 20 min", "45 min", "under 1 min" (and "0 min" for none). */
export function duration(seconds: number): string {
  if (seconds <= 0) return "0 min";
  const mins = Math.floor(seconds / 60);
  if (mins < 1) return "under 1 min";
  const h = Math.floor(mins / 60);
  const m = mins % 60;
  if (h === 0) return `${m} min`;
  return m === 0 ? `${h} h` : `${h} h ${m} min`;
}

export type RangeId = "7d" | "30d" | "90d" | "custom";
export const RANGE_DAYS = { "7d": 7, "30d": 30, "90d": 90 } as const;

/** A UTC date as `YYYY-MM-DD`. */
export const isoDay = (d: Date): string => d.toISOString().slice(0, 10);

const DAY_MS = 86_400_000;

/** `from`/`to` (UTC days, inclusive) of a preset ending today. */
export function presetRange(days: number, now = new Date()): { from: string; to: string } {
  const to = new Date(Date.UTC(now.getUTCFullYear(), now.getUTCMonth(), now.getUTCDate()));
  return { from: isoDay(new Date(to.getTime() - (days - 1) * DAY_MS)), to: isoDay(to) };
}

/** Days of an inclusive range, or `NaN` for an unparsable date. */
export function rangeDays(from: string, to: string): number {
  const a = Date.parse(`${from}T00:00:00Z`);
  const b = Date.parse(`${to}T00:00:00Z`);
  return Math.round((b - a) / DAY_MS) + 1;
}

/** Monday of the UTC week of a `YYYY-MM-DD` day. */
export function weekStart(day: string): string {
  const d = new Date(`${day}T00:00:00Z`);
  const back = (d.getUTCDay() + 6) % 7;
  return isoDay(new Date(d.getTime() - back * DAY_MS));
}

/** `Nov 17` for a `YYYY-MM-DD` day (UTC, no time zone shift). */
export function shortDay(day: string): string {
  return new Date(`${day}T00:00:00Z`).toLocaleDateString("en-US", { month: "short", day: "numeric", timeZone: "UTC" });
}

export interface DayPoint {
  day: string;
  turns: number;
  turn_seconds: number;
  awake_seconds: number;
  input_tokens: number;
  output_tokens: number;
}

export type Measure = "tokens" | "turns" | "turn_time" | "awake_time";

export const measureValue = (p: DayPoint, m: Measure): number =>
  m === "tokens" ? p.input_tokens + p.output_tokens : m === "turns" ? p.turns : m === "turn_time" ? p.turn_seconds : p.awake_seconds;

/** The day points summed per UTC week (Monday first); each keeps its week's first day in the range as `day`. */
export function groupByWeek<T extends DayPoint>(days: T[]): DayPoint[] {
  const weeks = new Map<string, DayPoint>();
  for (const d of days) {
    const key = weekStart(d.day);
    const w = weeks.get(key);
    if (w) {
      w.turns += d.turns;
      w.turn_seconds += d.turn_seconds;
      w.awake_seconds += d.awake_seconds;
      w.input_tokens += d.input_tokens;
      w.output_tokens += d.output_tokens;
    } else {
      weeks.set(key, {
        day: d.day,
        turns: d.turns,
        turn_seconds: d.turn_seconds,
        awake_seconds: d.awake_seconds,
        input_tokens: d.input_tokens,
        output_tokens: d.output_tokens,
      });
    }
  }
  return [...weeks.values()];
}
