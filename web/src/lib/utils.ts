import { clsx, type ClassValue } from "clsx";
import { extendTailwindMerge } from "tailwind-merge";

import { formatWhen } from "./format-date";

// The theme's own font sizes and tracking: the `--text-*` and `--tracking-*` variables in `src/app/globals.css`. Name
// a new one here too (`tests/cn.test.tsx` fails otherwise): tailwind-merge reads an unknown `text-label` as a colour and
// drops it next to a real one.
const twMerge = extendTailwindMerge({
  extend: { theme: { text: ["label", "micro"], tracking: ["label", "brand"] } },
});

export function cn(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs));
}

/** (P5-POLISH-2) A time as `formatWhen` says it ("5 min ago", "18 Nov"), or `empty` ("—" by default) when there is none. */
export const fmtDate = (s?: string | null, empty = "—") => formatWhen(s, { empty });

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
