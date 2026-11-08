"use client";

import type { FormEvent, ReactNode } from "react";

import { Button } from "./button";
import { Dialog, DialogClose, DialogContent } from "./dialog";

/**
 * Confirmation dialog. `danger` styles the confirm button as destructive. For a dialog with
 * fields, use `FormDialog`. The caller owns `open` and closes it in `onConfirm` when done.
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
  description?: string;
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
  onSubmit,
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
  onSubmit: (e: FormEvent<HTMLFormElement>) => void;
  children: ReactNode;
}) {
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent title={title} description={description}>
        <form onSubmit={onSubmit} className="space-y-4">
          {children}
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
