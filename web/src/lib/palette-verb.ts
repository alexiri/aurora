"use client";

import { useEffect, useRef } from "react";

/**
 * (P5-REFINE-19) How a palette verb or a shortcut reaches the dialog that its button opens. The palette navigates to the
 * page that owns the button and leaves a *pending verb*; the control that owns the dialog registers `useVerb(id, open)`
 * and opens it when it mounts (or at once, when it is already on screen). The verb never acts on its own: the dialog it
 * opens is the one the button opens, with the same confirmation. A verb nobody picks up in five seconds is dropped.
 */

export type VerbId = "new-agent" | "new-chat" | "new-schedule" | "new-webhook" | "pause-agent" | "export-agent" | "import-agent";

const TTL_MS = 5000;
let pending: { id: VerbId; at: number } | null = null;
const listeners = new Set<{ id: VerbId; run: () => void }>();

/** Ask for a verb. Runs at once when its control is mounted, else when it mounts. */
export function requestVerb(id: VerbId): void {
  for (const l of [...listeners]) {
    if (l.id === id) {
      l.run();
      pending = null;
      return;
    }
  }
  pending = { id, at: Date.now() };
}

/** Register the control that opens the dialog of `id`. `run` may change between renders. */
export function useVerb(id: VerbId, run: () => void, enabled = true): void {
  const latest = useRef(run);
  useEffect(() => {
    latest.current = run;
  });
  useEffect(() => {
    if (!enabled) return;
    const entry = { id, run: () => latest.current() };
    listeners.add(entry);
    if (pending && pending.id === id && Date.now() - pending.at < TTL_MS) {
      pending = null;
      entry.run();
    }
    return () => {
      listeners.delete(entry);
    };
  }, [id, enabled]);
}

/** Tests: forget a pending verb. */
export function resetVerbs(): void {
  pending = null;
}
