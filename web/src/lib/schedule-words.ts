import type { SchedulePreset } from "@/lib/cron";
import { zoneCity } from "@/lib/cron";

/** (P5-REFINE-14) Run times in words, in the schedule's time zone (the API gives instants in UTC, C-1077). */

function parts(instant: string | Date, timezone: string): { y: number; m: number; d: number; hh: string; mm: string; weekday: string; dayMonth: string } | null {
  const date = typeof instant === "string" ? new Date(instant) : instant;
  if (Number.isNaN(date.getTime())) return null;
  let f: Intl.DateTimeFormat;
  try {
    f = new Intl.DateTimeFormat("en-GB", { timeZone: timezone, year: "numeric", month: "numeric", day: "numeric", hour: "2-digit", minute: "2-digit", hourCycle: "h23", weekday: "short" });
  } catch {
    return null;
  }
  const p = Object.fromEntries(f.formatToParts(date).map((x) => [x.type, x.value]));
  const dayMonth = new Intl.DateTimeFormat("en-GB", { timeZone: timezone, day: "numeric", month: "short" }).format(date);
  return { y: Number(p.year), m: Number(p.month), d: Number(p.day), hh: p.hour, mm: p.minute, weekday: p.weekday, dayMonth };
}

/** "23:55" in `timezone`. */
export function clockIn(instant: string | Date, timezone: string): string {
  const p = parts(instant, timezone);
  return p ? `${p.hh}:${p.mm}` : "";
}

/** "today 23:55", "tomorrow 09:00", "Mon 24 Nov, 09:00": a run time for a sentence. */
export function runTimeWords(instant: string | Date, timezone: string, now: Date = new Date()): string {
  const p = parts(instant, timezone);
  const n = parts(now, timezone);
  if (!p || !n) return "";
  const dayOf = (x: { y: number; m: number; d: number }) => Date.UTC(x.y, x.m - 1, x.d) / 86_400_000;
  const diff = dayOf(p) - dayOf(n);
  const clock = `${p.hh}:${p.mm}`;
  if (diff === 0) return `today ${clock}`;
  if (diff === 1) return `tomorrow ${clock}`;
  return `${p.weekday} ${p.dayMonth}, ${clock}`;
}

/**
 * The summary line's second half: "next 23:55, 00:00, 00:05." for the short cadences, "next: tomorrow 09:00 (Zurich time)."
 * for the others. Empty when there are no instants (paused, or the preview has not answered).
 */
export function nextWords(preset: SchedulePreset, instants: string[], timezone: string, now: Date = new Date()): string {
  if (instants.length === 0) return "";
  if (preset === "every5" || preset === "hourly") return `next ${instants.map((i) => clockIn(i, timezone)).join(", ")}.`;
  const city = zoneCity(timezone);
  return `next: ${runTimeWords(instants[0], timezone, now)}${city ? ` (${city} time)` : ""}.`;
}
