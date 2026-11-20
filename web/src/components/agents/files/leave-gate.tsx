"use client";

import { useCallback, useState, type ReactNode } from "react";

import { LeaveDialog } from "./dialogs";

/**
 * (P5-REFINE-15, computer-4) The one place that knows whether the Files editor holds unsaved changes. The editor
 * registers itself while it is dirty (`setLeaveGuard`); every way out of it asks `useLeaveGate().ask(go)` first: Back to
 * folder, another file, the breadcrumb, and the agent page's own links (its sub-nav and sections).
 */
export interface LeaveGuard {
  name: string;
  /** Saves the file; resolves true when it was saved (a conflict or an error keeps the editor open: false). */
  save: () => Promise<boolean>;
  /** Marks the editor clean, so that the page can leave without the browser's own warning. */
  discard: () => void;
}

let guard: LeaveGuard | null = null;
export const setLeaveGuard = (g: LeaveGuard | null) => {
  guard = g;
};
export const getLeaveGuard = () => guard;

/** `ask(go)` runs `go` at once when nothing is unsaved, else asks "Leave without saving?" and runs it on Discard or Save and leave. */
export function useLeaveGate(): { ask: (go: () => void) => void; dialog: ReactNode } {
  const [pending, setPending] = useState<{ go: () => void; name: string } | null>(null);
  const ask = useCallback((go: () => void) => {
    const g = getLeaveGuard();
    if (g) setPending({ go, name: g.name });
    else go();
  }, []);
  const dialog = (
    <LeaveDialog
      name={pending?.name ?? null}
      onKeep={() => setPending(null)}
      onDiscard={() => {
        const p = pending;
        getLeaveGuard()?.discard();
        setPending(null);
        p?.go();
      }}
      onSaveAndLeave={async () => {
        const p = pending;
        const saved = (await getLeaveGuard()?.save()) ?? true;
        if (!saved) {
          setPending(null);
          return;
        }
        setPending(null);
        p?.go();
      }}
    />
  );
  return { ask, dialog };
}
