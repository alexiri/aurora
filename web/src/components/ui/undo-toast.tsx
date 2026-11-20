"use client";

import { useEffect, useSyncExternalStore } from "react";

import { Button } from "@/components/ui/button";

/** How long the toast stays (P5-REFINE-2: 6 seconds). */
export const UNDO_TOAST_MS = 6000;

export interface UndoToast {
  id: number;
  message: string;
  onUndo?: () => void;
}

let current: UndoToast | null = null;
let nextId = 1;
const listeners = new Set<() => void>();
const emit = () => listeners.forEach((l) => l());

/** Show a toast (one at a time: a new one replaces the old). `onUndo` adds the **Undo** button. */
export function showUndoToast(message: string, onUndo?: () => void): void {
  current = { id: nextId++, message, onUndo };
  emit();
}

export function dismissUndoToast(): void {
  current = null;
  emit();
}

/** (P5-REFINE-2, first rung of the confirm ladder) `const toast = useUndoToast(); toast("Webhook paused", undo)`. */
export function useUndoToast(): typeof showUndoToast {
  return showUndoToast;
}

/** The toast region at the bottom of the app; mounted once by the app shell. */
export function UndoToastHost() {
  const toast = useSyncExternalStore(
    (l) => {
      listeners.add(l);
      return () => {
        listeners.delete(l);
      };
    },
    () => current,
    () => null,
  );
  useEffect(() => {
    if (!toast) return;
    const t = setTimeout(() => {
      if (current?.id === toast.id) dismissUndoToast();
    }, UNDO_TOAST_MS);
    return () => clearTimeout(t);
  }, [toast]);
  if (!toast) return <div aria-live="polite" role="status" className="sr-only" />;
  return (
    <div
      role="status"
      aria-live="polite"
      data-testid="undo-toast"
      className="fixed inset-x-4 bottom-4 z-50 mx-auto flex max-w-md items-center justify-between gap-3 rounded-card border border-line bg-surface px-4 py-3 shadow-pop"
    >
      <span className="min-w-0 text-sm font-medium text-ink">{toast.message}</span>
      {toast.onUndo ? (
        <Button
          type="button"
          size="sm"
          variant="outline"
          onClick={() => {
            toast.onUndo?.();
            dismissUndoToast();
          }}
        >
          Undo
        </Button>
      ) : null}
    </div>
  );
}
