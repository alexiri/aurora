"use client";

import { useSyncExternalStore } from "react";

import { formatExact, formatWhen } from "@/lib/format-date";

/** How often relative times re-render (P5-REFINE-2: 30 seconds, one timer for the whole page). */
export const WHEN_TICK_MS = 30_000;

let now = Date.now();
const listeners = new Set<() => void>();
let timer: ReturnType<typeof setInterval> | null = null;

function tick() {
  now = Date.now();
  listeners.forEach((l) => l());
}

function onVisibility() {
  // Catch up at once when the tab comes back.
  if (document.visibilityState === "visible") tick();
}

function start() {
  if (timer) return;
  now = Date.now();
  timer = setInterval(() => {
    // Paused while the tab is hidden.
    if (typeof document !== "undefined" && document.visibilityState === "hidden") return;
    tick();
  }, WHEN_TICK_MS);
  if (typeof document !== "undefined") document.addEventListener("visibilitychange", onVisibility);
}

function stop() {
  if (timer) clearInterval(timer);
  timer = null;
  if (typeof document !== "undefined") document.removeEventListener("visibilitychange", onVisibility);
}

function subscribe(listener: () => void) {
  listeners.add(listener);
  start();
  return () => {
    listeners.delete(listener);
    if (listeners.size === 0) stop();
  };
}

/** The shared clock: re-renders its caller every 30 s through the one page-wide timer. */
export function useWhenNow(): number {
  return useSyncExternalStore(subscribe, () => now, () => now);
}

/**
 * (P5-REFINE-2, D-405) A time as `<time dateTime title={exact}>` showing `formatWhen`, kept current every 30 s without a
 * timer per row. A missing time shows `empty` ("—").
 */
export function When({ value, empty, className }: { value: string | number | Date | null | undefined; empty?: string; className?: string }) {
  const at = useWhenNow();
  const text = formatWhen(value, { now: at, empty });
  if (value === null || value === undefined || value === "") return <span className={className}>{text}</span>;
  const iso = value instanceof Date ? value.toISOString() : typeof value === "number" ? new Date(value).toISOString() : value;
  return (
    <time dateTime={iso} title={formatExact(value)} className={className}>
      {text}
    </time>
  );
}
