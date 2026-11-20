"use client";

import { useSyncExternalStore } from "react";

// (P5-REFINE-11, chat-4, C-1075) Turns the person pressed Stop on. The header goes back to Ready and the Stop button reads
// "Stopping…" at once, without waiting for `turn.finished`; the durable `turn_stopped` notice follows from the API.

const MAX = 50;
let stopped: ReadonlySet<string> = new Set();
const listeners = new Set<() => void>();
const emit = () => listeners.forEach((l) => l());

/** Remember that Stop was pressed for this turn. */
export function markStopped(turnId: string): void {
  const next = new Set(stopped);
  next.add(turnId);
  // Turn ids never come back: keep only the newest few.
  while (next.size > MAX) next.delete(next.values().next().value as string);
  stopped = next;
  emit();
}

/** Tests: forget every stop. */
export function resetStopped(): void {
  stopped = new Set();
  emit();
}

const empty: ReadonlySet<string> = new Set();

export function useStoppedTurns(): ReadonlySet<string> {
  return useSyncExternalStore(
    (l) => {
      listeners.add(l);
      return () => {
        listeners.delete(l);
      };
    },
    () => stopped,
    () => empty,
  );
}
