"use client";

import * as P from "@radix-ui/react-popover";
import { useId, useRef, useState, type ComponentProps, type ReactNode } from "react";

import { cn } from "@/lib/utils";

/**
 * Popover (docs/14 §Visual design): a small non-modal panel anchored to a trigger, e.g. the status rail's Needs-you
 * items. Esc closes it and returns focus to the trigger; clicking outside closes it.
 */
export const Popover = P.Root;
export const PopoverTrigger = P.Trigger;
export const PopoverAnchor = P.Anchor;

export function PopoverContent({ className, align = "start", sideOffset = 8, ...props }: ComponentProps<typeof P.Content>) {
  return (
    <P.Portal>
      <P.Content
        align={align}
        sideOffset={sideOffset}
        className={cn(
          "z-50 max-h-96 w-80 max-w-full overflow-y-auto rounded-card border border-line bg-surface p-3 text-ink shadow-pop outline-none",
          className,
        )}
        {...props}
      />
    </P.Portal>
  );
}

/**
 * (retention-tooltip) A sentence-sized note attached to a piece of inline text: the note shows on pointer hover,
 * keyboard focus and a touch tap, and Escape, blur and a tap or press outside close it. While it shows, the note is
 * the trigger's accessible description (`aria-describedby`, the content is `role="tooltip"`); the trigger keeps its
 * own text, so the note never carries the control's only name. A mouse-out waits briefly, so the pointer can cross
 * the gap to the note. On the Popover, so it keeps that surface (`surface`, `line` border, `shadow-pop`): tokens only.
 */
export function InfoTip({
  note,
  children,
  className,
  contentClassName,
  testId,
}: {
  /** The note: the tooltip's text and, while it shows, the trigger's accessible description. */
  note: ReactNode;
  /** The trigger's own text (its accessible name). */
  children: ReactNode;
  className?: string;
  contentClassName?: string;
  testId?: string;
}) {
  const [open, setOpen] = useState(false);
  const id = useId();
  const hide = useRef<ReturnType<typeof setTimeout> | null>(null);
  const cancelHide = () => {
    if (hide.current !== null) clearTimeout(hide.current);
    hide.current = null;
  };
  const laterHide = () => {
    cancelHide();
    hide.current = setTimeout(() => setOpen(false), 150);
  };
  return (
    <P.Root open={open} onOpenChange={setOpen}>
      <P.Anchor asChild>
        {/* A `span role="button"`, not a `button`: Chrome lays a button out as an atomic inline-block however it
            displays, so the line's text would wrap as one piece and overflow the 64 px header (agent-page-p5). An
            inline span fragments, so the line wraps exactly as the plain text does. Focusable, so focus opens. */}
        <span
          role="button"
          tabIndex={0}
          className={cn(
            "inline cursor-help text-left underline decoration-dotted decoration-line underline-offset-2 hover:decoration-line-strong focus-visible:outline focus-visible:outline-2 focus-visible:outline-accent",
            className,
          )}
          aria-describedby={open ? id : undefined}
          onFocus={() => {
            cancelHide();
            setOpen(true);
          }}
          onBlur={() => {
            cancelHide();
            setOpen(false);
          }}
          onMouseEnter={() => {
            cancelHide();
            setOpen(true);
          }}
          onMouseLeave={laterHide}
          // A touch tap focuses and clicks; a tap or press outside closes (blur and the Popover's outside dismissal).
          onClick={() => {
            cancelHide();
            setOpen(true);
          }}
          onKeyDown={(e) => {
            if (e.key === "Escape") {
              cancelHide();
              setOpen(false);
            } else if (e.key === "Enter" || e.key === " ") {
              // A button's keys, though focus has already opened the note.
              e.preventDefault();
              setOpen(true);
            }
          }}>
          {children}
        </span>
      </P.Anchor>
      <PopoverContent
        id={id}
        role="tooltip"
        sideOffset={6}
        data-testid={testId}
        className={cn("w-auto max-w-80 p-2 text-xs", contentClassName)}
        // A tooltip never takes the focus: Radix's FocusScope would move it into the panel on open and
        // back to "where focus was" on close, which blurs the line the note belongs to (and closes it).
        onOpenAutoFocus={(e) => e.preventDefault()}
        onCloseAutoFocus={(e) => e.preventDefault()}
        onMouseEnter={cancelHide}
        onMouseLeave={laterHide}>
        {note}
      </PopoverContent>
    </P.Root>
  );
}
