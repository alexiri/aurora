/**
 * (P5-POLISH-2, D-384; P5-REFINE-2, D-405) The one way to show a time. `formatWhen`: "Just now", "5 min ago", "2 h ago",
 * then the dated form "19 Nov, 23:50" (with the year when it is not this year: "19 Nov 2025, 23:50") in the person's
 * locale and time zone. `formatExact` is the `title` tooltip: locale date and time to the minute, no seconds. Both give
 * "—" for a missing or invalid time. `formatDay` is a date without a time ("19 Nov 2026"); `formatTime` a time only.
 *
 * The person's time zone (`GET /v1/me` `timezone`, C-1054) is set once by the app shell through `setDisplayTimeZone`;
 * until then, and when it is empty, the browser's own zone is used.
 */
const EMPTY = "—";

let zone: string | undefined;

/** Set the time zone every time is shown in (an IANA name); `null` goes back to the browser's. */
export function setDisplayTimeZone(tz: string | null | undefined): void {
  if (!tz) {
    zone = undefined;
    return;
  }
  try {
    new Intl.DateTimeFormat(undefined, { timeZone: tz });
    zone = tz;
  } catch {
    zone = undefined;
  }
}

/** The zone times are shown in. */
export function displayTimeZone(): string | undefined {
  return zone;
}

/**
 * (P5-REFINE-20, b2#18) The zone as a name every formatter accepts: the person's, else the browser's, else UTC. One zone on
 * a screen: a schedule's own zone is said only where it is the schedule's setting ("runs at 09:00 Zurich time").
 */
export function personTimeZone(): string {
  if (zone) return zone;
  try {
    return Intl.DateTimeFormat().resolvedOptions().timeZone || "UTC";
  } catch {
    return "UTC";
  }
}

function parse(date: string | number | Date | null | undefined): Date | null {
  if (date === null || date === undefined || date === "") return null;
  const d = date instanceof Date ? date : new Date(date);
  return Number.isNaN(d.getTime()) ? null : d;
}

function yearOf(d: Date): number {
  return Number(new Intl.DateTimeFormat("en-US", { year: "numeric", timeZone: zone }).format(d));
}

export function formatWhen(
  date: string | number | Date | null | undefined,
  { now = Date.now(), empty = EMPTY }: { now?: number; empty?: string } = {},
): string {
  const d = parse(date);
  if (!d) return empty;
  const secs = Math.round((now - d.getTime()) / 1000);
  if (secs < 60 && secs > -60) return "Just now";
  if (secs >= 60 && secs < 3600) return `${Math.floor(secs / 60)} min ago`;
  if (secs >= 3600 && secs < 86400) return `${Math.floor(secs / 3600)} h ago`;
  const sameYear = yearOf(d) === yearOf(new Date(now));
  const dayPart = new Intl.DateTimeFormat(undefined, {
    day: "numeric",
    month: "short",
    ...(sameYear ? {} : { year: "numeric" }),
    timeZone: zone,
  }).format(d);
  return `${dayPart}, ${formatTime(d)}`;
}

/** `formatWhen` inside a sentence ("last just now", "Since just now"): the only word with a capital of its own is lower-cased. */
export function whenInSentence(date: string | number | Date | null | undefined, opts?: { now?: number; empty?: string }): string {
  const when = formatWhen(date, opts);
  return when === "Just now" ? "just now" : when;
}

/** A 24-hour clock time ("23:50") in the person's zone, or in `timeZone` when given; `empty` when missing. */
export function formatTime(date: string | number | Date | null | undefined, empty: string = EMPTY, timeZone: string | undefined = zone): string {
  const d = parse(date);
  return d ? new Intl.DateTimeFormat(undefined, { hour: "2-digit", minute: "2-digit", hourCycle: "h23", timeZone }).format(d) : empty;
}

/** The calendar day `d` falls on in the person's zone, as a day number (for counting days apart). */
function dayNumber(d: Date): number {
  const parts = new Intl.DateTimeFormat("en-US", { year: "numeric", month: "numeric", day: "numeric", timeZone: zone }).formatToParts(d);
  const n = (type: string) => Number(parts.find((p) => p.type === type)?.value ?? 0);
  return Date.UTC(n("year"), n("month") - 1, n("day")) / 86_400_000;
}

const plural = (n: number, unit: string) => `${n} ${unit}${n === 1 ? "" : "s"}`;

/**
 * (P5-MEMORY-2) How long ago, in calendar days, for a line that says where something came from: "today", "yesterday",
 * "2 days ago", "1 week ago", "3 weeks ago", "2 months ago", "1 year ago". Empty for a missing time. Show it in a
 * `<time>` whose title is `formatExact`.
 */
export function formatAge(date: string | number | Date | null | undefined, now: number = Date.now()): string {
  const d = parse(date);
  if (!d) return "";
  const days = Math.max(0, Math.round(dayNumber(new Date(now)) - dayNumber(d)));
  if (days === 0) return "today";
  if (days === 1) return "yesterday";
  if (days < 7) return `${days} days ago`;
  if (days < 60) return `${plural(Math.floor(days / 7), "week")} ago`;
  if (days < 365) return `${plural(Math.floor(days / 30), "month")} ago`;
  return `${plural(Math.floor(days / 365), "year")} ago`;
}

/** A day inside a sentence: "25 Nov", with the year when it is not this year; "" when missing. */
export function formatShortDay(date: string | number | Date | null | undefined, now: number = Date.now()): string {
  const d = parse(date);
  if (!d) return "";
  const sameYear = yearOf(d) === yearOf(new Date(now));
  return new Intl.DateTimeFormat(undefined, { day: "numeric", month: "short", ...(sameYear ? {} : { year: "numeric" }), timeZone: zone }).format(d);
}

/** A date without a time ("19 Nov 2026"), "—" when missing. */
export function formatDay(date: string | number | Date | null | undefined, empty: string = EMPTY): string {
  const d = parse(date);
  return d ? new Intl.DateTimeFormat(undefined, { day: "numeric", month: "short", year: "numeric", timeZone: zone }).format(d) : empty;
}

export function formatExact(date: string | number | Date | null | undefined, empty: string = EMPTY): string {
  const d = parse(date);
  return d ? new Intl.DateTimeFormat(undefined, { dateStyle: "medium", timeStyle: "short", hourCycle: "h23", timeZone: zone }).format(d) : empty;
}

/** A whole number with the locale's grouping ("1,234"). */
export function formatCount(n: number): string {
  return new Intl.NumberFormat(undefined).format(n);
}
