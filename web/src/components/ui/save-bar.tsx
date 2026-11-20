"use client";

import { useCallback, useEffect, useRef, useState, type ReactNode } from "react";

import { Button } from "@/components/ui/button";
import { CheckIcon } from "@/components/ui/icons";

/** How long "Saved" stays (P5-REFINE-2: 2 seconds). */
export const SAVED_MS = 2000;

/**
 * (P5-REFINE-2, priority 2) State of the "Saved" flash for a `SaveBar` or an `InlineSaved`: `saved` is true for two
 * seconds after `flash()`. `clear()` hides it at once (a new edit).
 */
export function useSaveBar(ms: number = SAVED_MS): { saved: boolean; flash: () => void; clear: () => void } {
  const [saved, setSaved] = useState(false);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const clear = useCallback(() => {
    if (timer.current) clearTimeout(timer.current);
    timer.current = null;
    setSaved(false);
  }, []);
  const flash = useCallback(() => {
    if (timer.current) clearTimeout(timer.current);
    setSaved(true);
    timer.current = setTimeout(() => {
      timer.current = null;
      setSaved(false);
    }, ms);
  }, [ms]);
  useEffect(
    () => () => {
      if (timer.current) clearTimeout(timer.current);
    },
    [],
  );
  return { saved, flash, clear };
}

/** Scroll to the first field that is marked invalid and focus it. */
export function focusFirstInvalid(root: ParentNode = document): void {
  const el = root.querySelector<HTMLElement>('[aria-invalid="true"]');
  if (!el) return;
  el.scrollIntoView?.({ block: "center", behavior: "smooth" });
  el.focus({ preventScroll: true });
}

/**
 * (P5-REFINE-2, priority 2; admin-2, behaviour-6) The one save pattern of every settings form. A bar fixed at the bottom
 * of the viewport, hidden until the first change:
 *
 * - **Unsaved changes** with **Discard** and **Save** (`unsavedLabel` replaces the words, e.g. "2 unsaved changes");
 * - **Saving…** (Save disabled);
 * - **Saved** (or `savedMessage`) with an optional **Undo** while `saved` is true (see `useSaveBar`);
 * - **n fields need fixing** while `errorCount > 0` (the page scrolls to the first bad field and focuses it), or the
 *   failure's own sentence in `error` with Save still enabled.
 *
 * Without `onSave` the Save button is a submit button of the surrounding form. `children` is an extra sentence.
 */
export function SaveBar({
  dirty,
  saving = false,
  saved = false,
  errorCount = 0,
  error,
  onSave,
  onDiscard,
  onUndo,
  savedMessage = "Saved",
  unsavedLabel = "Unsaved changes",
  saveLabel = "Save",
  children,
}: {
  dirty: boolean;
  saving?: boolean;
  saved?: boolean;
  /** How many fields have an error. */
  errorCount?: number;
  /** A failure that is not a field error. */
  error?: ReactNode;
  onSave?: () => void;
  onDiscard: () => void;
  onUndo?: () => void;
  savedMessage?: string;
  unsavedLabel?: string;
  saveLabel?: string;
  children?: ReactNode;
}) {
  // After a failed save, go to the first bad field.
  const wasSaving = useRef(false);
  useEffect(() => {
    if (wasSaving.current && !saving && errorCount > 0) focusFirstInvalid();
    wasSaving.current = saving;
  }, [saving, errorCount]);

  const shell =
    "fixed inset-x-4 bottom-4 z-30 mx-auto flex max-w-xl items-center justify-between gap-3 rounded-card border border-line bg-surface px-4 py-3 shadow-pop";
  // "Saved" shows once nothing is pending again (a new edit makes the bar the unsaved one at once).
  if (saved && !dirty) {
    return (
      <div className={shell} data-testid="save-bar-saved">
        <span role="status" aria-live="polite" className="flex min-w-0 items-center gap-2 text-sm font-medium text-ink">
          <CheckIcon aria-hidden="true" className="size-4 shrink-0 text-accent-ink" />
          {savedMessage}
        </span>
        {onUndo ? (
          <Button type="button" variant="outline" size="sm" onClick={onUndo}>
            Undo
          </Button>
        ) : null}
      </div>
    );
  }
  if (!dirty) return null;
  const message =
    errorCount > 0 ? (errorCount === 1 ? "1 field needs fixing" : `${errorCount} fields need fixing`) : error ?? unsavedLabel;
  const failed = errorCount > 0 || Boolean(error);
  return (
    <div role="region" aria-label="Unsaved changes" className={shell}>
      <span className={failed ? "min-w-0 text-sm font-medium text-danger-ink" : "min-w-0 text-sm font-medium text-ink"} role={failed ? "alert" : undefined}>
        {message}
        {children ? <span className="mt-0.5 block text-xs font-normal text-ink-2">{children}</span> : null}
      </span>
      <span className="flex shrink-0 items-center gap-2">
        <Button type="button" variant="outline" size="sm" onClick={onDiscard} disabled={saving}>
          Discard
        </Button>
        <Button type={onSave ? "button" : "submit"} size="sm" onClick={onSave} disabled={saving}>
          {saving ? "Saving…" : saveLabel}
        </Button>
      </span>
    </div>
  );
}

/**
 * (P5-REFINE-2) A small "Saved" (check + word) beside an instant control (a switch, a select in a row, the avatar):
 * shown while `show` is true (`useSaveBar().saved`), announced politely.
 */
export function InlineSaved({ show, children = "Saved" }: { show: boolean; children?: ReactNode }) {
  return (
    <span role="status" aria-live="polite" className="inline-flex min-h-5 items-center gap-1 text-xs font-medium text-accent-ink">
      {show ? (
        <>
          <CheckIcon aria-hidden="true" className="size-3.5" />
          {children}
        </>
      ) : null}
    </span>
  );
}
