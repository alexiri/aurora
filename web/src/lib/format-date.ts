/**
 * (P5-POLISH-2, D-384) The one way to show a time. `formatWhen`: "Just now", "5 min ago", "2 h ago", "Yesterday", or a
 * short date in the browser's locale ("18 Nov", with the year when it is not this year). `formatExact` is the
 * `title` tooltip: locale date and time to the minute, no seconds. Both give "—" for a missing or invalid time.
 */
const EMPTY = "—";

function parse(date: string | number | Date | null | undefined): Date | null {
  if (date === null || date === undefined || date === "") return null;
  const d = date instanceof Date ? date : new Date(date);
  return Number.isNaN(d.getTime()) ? null : d;
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
  const today = new Date(now);
  const startOfToday = new Date(today.getFullYear(), today.getMonth(), today.getDate()).getTime();
  if (d.getTime() >= startOfToday - 86_400_000 && d.getTime() < startOfToday) return "Yesterday";
  const sameYear = d.getFullYear() === today.getFullYear();
  return d.toLocaleDateString(undefined, sameYear ? { day: "numeric", month: "short" } : { day: "numeric", month: "short", year: "numeric" });
}

export function formatExact(date: string | number | Date | null | undefined, empty: string = EMPTY): string {
  const d = parse(date);
  return d ? d.toLocaleString(undefined, { dateStyle: "medium", timeStyle: "short" }) : empty;
}
