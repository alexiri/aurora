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

let workspaceZone: string | undefined;

/** (P5-REFINE-20, item 18) The current workspace's time zone, set once by the app shell; `null` clears it. */
export function setWorkspaceTimeZone(tz: string | null | undefined): void {
  if (!tz) {
    workspaceZone = undefined;
    return;
  }
  try {
    new Intl.DateTimeFormat(undefined, { timeZone: tz });
    workspaceZone = tz;
  } catch {
    workspaceZone = undefined;
  }
}

/**
 * The zone of what a workspace's automations did: schedule rows, run-thread titles and the delivery message all say their
 * times in the workspace's zone (never the browser's), falling back to the person's own when no workspace is known.
 */
export function automationTimeZone(): string | undefined {
  return workspaceZone ?? zone;
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

/** A 24-hour clock time ("23:50") in the person's zone, or in `timeZone` when given; `empty` when missing. */
export function formatTime(date: string | number | Date | null | undefined, empty: string = EMPTY, timeZone: string | undefined = zone): string {
  const d = parse(date);
  return d ? new Intl.DateTimeFormat(undefined, { hour: "2-digit", minute: "2-digit", hourCycle: "h23", timeZone }).format(d) : empty;
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
