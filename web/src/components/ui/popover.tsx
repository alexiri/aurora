"use client";

import * as P from "@radix-ui/react-popover";
import type { ComponentProps } from "react";

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
