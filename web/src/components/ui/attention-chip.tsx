import type { ReactNode } from "react";

import { cn } from "@/lib/utils";

/**
 * (P5-POLISH-2, D-384, F2) The only amber element: something needs the viewer, so an action sits next to it. Give the
 * action as `action` (a button or link, rendered right after the chip) or, when the page already has one nearby, its
 * element id as `actionId` (the chip then names it for assistive technology). There is no chip without an action.
 * Amber never says who owns a value (that is `LockPill`) or a status without an action (that is `Badge`).
 */
export type AttentionChipProps = {
  children: ReactNode;
  className?: string;
  "aria-hidden"?: boolean | "true" | "false";
  "aria-label"?: string;
  "data-testid"?: string;
} & ({ action: ReactNode; actionId?: undefined } | { actionId: string; action?: undefined });

export function AttentionChip({ children, action, actionId, className, ...aria }: AttentionChipProps) {
  const chip = (
    <span
      {...aria}
      data-attention-for={actionId}
      className={cn("inline-flex items-center gap-1 rounded-full bg-pending-soft px-2 py-0.5 text-label font-medium leading-4 text-pending-ink", className)}
    >
      {children}
    </span>
  );
  if (action === undefined) return chip;
  return (
    <span className="inline-flex flex-wrap items-center gap-2">
      {chip}
      {action}
    </span>
  );
}
