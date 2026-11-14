"use client";

import * as DialogPrimitive from "@radix-ui/react-dialog";
import type { ComponentProps, ReactNode } from "react";

import { cn } from "@/lib/utils";

import { XIcon } from "./icons";

export const Dialog = DialogPrimitive.Root;
export const DialogTrigger = DialogPrimitive.Trigger;
export const DialogClose = DialogPrimitive.Close;

/**
 * Dialog widths (docs/14 §Visual design): the width is capped at the viewport minus a 1 rem gutter.
 * `sm` (28 rem) is the default for confirms and short forms, `md` for pickers, `lg` for editors,
 * `xl` and `2xl` for previews and diffs.
 */
export type DialogSize = "sm" | "md" | "lg" | "xl" | "2xl";
const SIZE: Record<DialogSize, string> = {
  sm: "w-[min(28rem,calc(100vw-2rem))]",
  md: "w-[min(34rem,calc(100vw-2rem))]",
  lg: "w-[min(40rem,calc(100vw-2rem))]",
  xl: "w-[min(48rem,calc(100vw-2rem))]",
  "2xl": "w-[min(56rem,calc(100vw-2rem))]",
};

export function DialogContent({
  title,
  description,
  children,
  className,
  size = "sm",
  ...props
}: { title: string; description?: string; children: ReactNode; size?: DialogSize } & ComponentProps<
  typeof DialogPrimitive.Content
>) {
  return (
    <DialogPrimitive.Portal>
      <DialogPrimitive.Overlay className="fixed inset-0 z-40 bg-scrim" />
      <DialogPrimitive.Content
        className={cn(
          "fixed left-1/2 top-1/2 z-50 max-h-[calc(100dvh-2rem)] -translate-x-1/2 -translate-y-1/2 overflow-y-auto rounded-card border border-line bg-surface p-6 text-ink shadow-pop",
          SIZE[size],
          className,
        )}
        {...props}
      >
        {/* (FIX-149, D-272) The title row stays at the top of the scroll area so the close icon is always reachable. */}
        <div className="sticky top-0 z-10 -mx-6 -mt-6 flex items-start justify-between gap-3 bg-surface px-6 pb-3 pt-6">
          <div className="min-w-0">
            <DialogPrimitive.Title className="font-display text-lg font-semibold">{title}</DialogPrimitive.Title>
            {description ? (
              <DialogPrimitive.Description className="mt-1 text-sm text-ink-2">
                {description}
              </DialogPrimitive.Description>
            ) : (
              <DialogPrimitive.Description className="sr-only">{title}</DialogPrimitive.Description>
            )}
          </div>
          <DialogPrimitive.Close asChild>
            <button
              type="button"
              aria-label="Close"
              className="-mr-2 -mt-1 inline-flex size-9 shrink-0 items-center justify-center rounded-control text-ink-2 hover:bg-subtle hover:text-ink pointer-coarse:size-11"
            >
              <XIcon className="size-5" />
            </button>
          </DialogPrimitive.Close>
        </div>
        <div className="mt-1">{children}</div>
      </DialogPrimitive.Content>
    </DialogPrimitive.Portal>
  );
}
