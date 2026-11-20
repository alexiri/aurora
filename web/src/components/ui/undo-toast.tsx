"use client";

import Link from "next/link";
import { useEffect, useSyncExternalStore } from "react";

import { Button } from "@/components/ui/button";
import { CheckIcon } from "@/components/ui/icons";

/** How long the toast stays (P5-REFINE-2: 6 seconds). */
export const UNDO_TOAST_MS = 6000;

/** A link inside the toast ("Started · Open the conversation"); following it closes the toast. */
export interface ToastLink {
  label: string;
  href: string;
}

export interface UndoToast {
  id: number;
  message: string;
  onUndo?: () => void;
  /** (P5-REFINE-14) Shown after the message, after " · ". */
  link?: ToastLink;
  /** (P5-REFINE-14) A check mark before the message: something started or finished. */
  success?: boolean;
}

let current: UndoToast | null = null;
let nextId = 1;
const listeners = new Set<() => void>();
const emit = () => listeners.forEach((l) => l());

/** Show a toast (one at a time: a new one replaces the old). `onUndo` adds the **Undo** button. */
export function showUndoToast(message: string, onUndo?: () => void, extra?: Pick<UndoToast, "link" | "success">): void {
  current = { id: nextId++, message, onUndo, ...extra };
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
      <span className="flex min-w-0 items-center gap-2 text-sm font-medium text-ink">
        {toast.success ? <CheckIcon aria-hidden className="size-4 shrink-0 text-accent-ink" /> : null}
        <span className="min-w-0">
          {toast.message}
          {toast.link ? (
            <>
              {" · "}
              <Link href={toast.link.href} onClick={dismissUndoToast} className="text-accent-ink underline underline-offset-2 hover:no-underline">
                {toast.link.label}
              </Link>
            </>
          ) : null}
        </span>
      </span>
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
