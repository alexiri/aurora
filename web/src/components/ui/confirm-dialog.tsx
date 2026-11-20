"use client";

import { useState, type FormEvent, type ReactNode } from "react";

import { Button } from "./button";
import { Dialog, DialogClose, DialogContent, type DialogSize } from "./dialog";
import { ErrorNotice } from "./notice";

/**
 * Confirmation dialog, the second rung of the confirm ladder (P5-REFINE-2): reversible but disruptive actions (rotate,
 * promote, pause, remove). `description` is **required**: the consequence in a sentence ("The old URL stops working
 * now."). `confirmLabel` is the verb ("Rotate URL"); `danger` styles it as destructive when something stops working.
 * For a dialog with fields, use `FormDialog`; for a permanent delete, `TypeToConfirm`. The caller owns `open` and
 * closes it in `onConfirm` when done.
 */
export function ConfirmDialog({
  open,
  onOpenChange,
  title,
  description,
  confirmLabel,
  danger = false,
  busy = false,
  onConfirm,
  children,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  title: string;
  description: string;
  confirmLabel: string;
  danger?: boolean;
  busy?: boolean;
  onConfirm: () => void;
  children?: ReactNode;
}) {
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent title={title} description={description}>
        {children}
        <div className="mt-4 flex flex-wrap justify-end gap-2">
          <DialogClose asChild>
            <Button variant="outline">Cancel</Button>
          </DialogClose>
          <Button variant={danger ? "danger" : "primary"} disabled={busy} onClick={onConfirm}>
            {confirmLabel}
          </Button>
        </div>
      </DialogContent>
    </Dialog>
  );
}

/** Dialog around a form: fields as children, Cancel and a submit button in the footer. */
export function FormDialog({
  open,
  onOpenChange,
  title,
  description,
  submitLabel,
  danger = false,
  busy = false,
  submitDisabled = false,
  size,
  onSubmit,
  error,
  children,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  title: string;
  description?: string;
  submitLabel: string;
  danger?: boolean;
  busy?: boolean;
  submitDisabled?: boolean;
  /** Dialog width; a form with many fields uses `xl`. */
  size?: DialogSize;
  onSubmit: (e: FormEvent<HTMLFormElement>) => void;
  /** A failure of the submit: shown in a reserved one-line slot above the actions, so it does not push the form down. */
  error?: unknown;
  children: ReactNode;
}) {
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent title={title} description={description} size={size}>
        <form onSubmit={onSubmit} className="space-y-4">
          {children}
          <div className="min-h-6" data-testid="form-error-slot">
            <ErrorNotice error={error} />
          </div>
          <div className="flex flex-wrap justify-end gap-2">
            <DialogClose asChild>
              <Button type="button" variant="outline">
                Cancel
              </Button>
            </DialogClose>
            <Button type="submit" variant={danger ? "danger" : "primary"} disabled={busy || submitDisabled}>
              {submitLabel}
            </Button>
          </div>
        </form>
      </DialogContent>
    </Dialog>
  );
}

/**
 * (P5-REFINE-2) A button that opens a `ConfirmDialog` (the replacement of the retired inline `ConfirmButton`): the
 * trigger, the consequence sentence and the verb are all required. The dialog closes when `onConfirm` is called; a failed
 * action shows its own error where the page shows it.
 */
export function ConfirmAction({
  label,
  size,
  variant = "danger",
  title,
  description,
  confirmLabel,
  danger = true,
  pending = false,
  onConfirm,
  children,
}: {
  label: string;
  size?: "sm" | "default" | "lg";
  variant?: "danger" | "outline" | "ghost" | "primary";
  title: string;
  description: string;
  confirmLabel: string;
  danger?: boolean;
  pending?: boolean;
  onConfirm: () => void;
  children?: ReactNode;
}) {
  const [open, setOpen] = useState(false);
  return (
    <>
      <Button type="button" variant={variant} size={size} onClick={() => setOpen(true)}>
        {label}
      </Button>
      <ConfirmDialog
        open={open}
        onOpenChange={setOpen}
        title={title}
        description={description}
        confirmLabel={confirmLabel}
        danger={danger}
        busy={pending}
        onConfirm={() => {
          setOpen(false);
          onConfirm();
        }}
      >
        {children}
      </ConfirmDialog>
    </>
  );
}
