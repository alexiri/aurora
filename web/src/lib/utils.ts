import { clsx, type ClassValue } from "clsx";
import { twMerge } from "tailwind-merge";

export function cn(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs));
}

/** A timestamp in the browser's locale, or `empty` ("—" by default) when there is none. */
export const fmtDate = (s?: string | null, empty = "—") => (s ? new Date(s).toLocaleString() : empty);

/** "1 agent", "3 agents"; `many` defaults to `one` + "s". */
export const plural = (n: number, one: string, many = `${one}s`) => `${n} ${n === 1 ? one : many}`;

/** How long ago `iso` was at `now` (ms): "just now", "5 min ago", "3 h ago", "2 d ago". */
export function ago(iso: string, now: number): string {
  const s = Math.max(0, Math.floor((now - new Date(iso).getTime()) / 1000));
  if (s < 60) return "just now";
  const m = Math.floor(s / 60);
  if (m < 60) return `${m} min ago`;
  const h = Math.floor(m / 60);
  return h < 24 ? `${h} h ago` : `${Math.floor(h / 24)} d ago`;
}
