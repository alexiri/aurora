"use client";

import * as DialogPrimitive from "@radix-ui/react-dialog";
import { useCallback, useRef, type ComponentProps, type ReactNode } from "react";

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

/** `sheet` is a full-screen panel below 640 px and the same as `centered` above it. */
export type DialogVariant = "centered" | "sheet";
const SHEET =
  "max-sm:left-0 max-sm:top-0 max-sm:h-dvh max-sm:max-h-none max-sm:w-screen max-sm:translate-x-0 max-sm:translate-y-0 max-sm:rounded-none max-sm:border-0";

/**
 * (P5-POLISH-1, D-393) Three rows in a flex column that never scrolls as a whole: a header (title, description, the
 * optional `header` slot such as a stepper, and the close button), a body (the only element that scrolls) and an
 * optional `footer` with a top border. `fixedHeight` ("600px") pins the height on desktop so multi-step dialogs do not
 * re-centre between steps; `back` is a slot before the title (the sheet's back button).
 */
export function DialogContent({
  title,
  description,
  children,
  className,
  size = "sm",
  header,
  footer,
  back,
  variant = "centered",
  fixedHeight,
  bodyClassName,
  ...props
}: {
  /** For a body laid out as columns that scroll on their own: it replaces the body's padding and scrolling. */
  bodyClassName?: string;
  title: string;
  description?: string;
  children: ReactNode;
  size?: DialogSize;
  header?: ReactNode;
  footer?: ReactNode;
  back?: ReactNode;
  variant?: DialogVariant;
  fixedHeight?: string;
} & ComponentProps<typeof DialogPrimitive.Content>) {
  // Set through the CSSOM: a `style` attribute is not allowed by the CSP guard, a property set from script is.
  const heightRef = useCallback(
    (el: HTMLDivElement | null) => {
      if (el && fixedHeight && window.matchMedia?.("(min-width: 640px)").matches) el.style.height = fixedHeight;
    },
    [fixedHeight],
  );
  const bodyRef = useRef<HTMLDivElement>(null);
  // (P5-POLISH-1 review) Radix focuses the first tabbable element, which is Close (it comes first in the header): a
  // dialog opened from a menu then shows a focus ring on Close. Focus the body's first field, else the content.
  const { onOpenAutoFocus, ...contentProps } = props;
  const openAutoFocus = (event: Event) => {
    onOpenAutoFocus?.(event);
    if (event.defaultPrevented) return;
    event.preventDefault();
    const field = bodyRef.current?.querySelector<HTMLElement>(
      "input:not([type=hidden]):not(:disabled), textarea:not(:disabled), select:not(:disabled)",
    );
    (field ?? (event.currentTarget as HTMLElement | null))?.focus();
  };
  return (
    <DialogPrimitive.Portal>
      <DialogPrimitive.Overlay className="fixed inset-0 z-40 bg-scrim" />
      <DialogPrimitive.Content
        ref={heightRef}
        className={cn(
          // (P5-REFINE-2) Top-anchored at a fixed offset (12vh from 640 px, 1rem below), never vertically centred: a field
          // or an error that appears does not move the title. `max-h` keeps the bottom 1rem free.
          "fixed left-1/2 top-4 z-50 flex max-h-[calc(100dvh-2rem)] -translate-x-1/2 flex-col sm:top-[12vh] sm:max-h-[calc(88dvh-1rem)] overflow-hidden rounded-card border border-line bg-surface text-ink shadow-pop outline-none",
          SIZE[size],
          variant === "sheet" && SHEET,
          className,
        )}
        onOpenAutoFocus={openAutoFocus}
        {...contentProps}
      >
        <div className="shrink-0 px-6 pb-3 pt-6">
          <div className="flex items-start justify-between gap-3">
            {back ? <div className="-ml-2 -mt-1 shrink-0">{back}</div> : null}
            <div className="min-w-0 flex-1">
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
          {header ? <div className="mt-3">{header}</div> : null}
        </div>
        <div ref={bodyRef} className={cn("min-h-0 flex-1 overflow-y-auto px-6 py-4", bodyClassName)}>
          {children}
        </div>
        {footer ? <div className="shrink-0 border-t border-line px-6 py-3">{footer}</div> : null}
      </DialogPrimitive.Content>
    </DialogPrimitive.Portal>
  );
}
