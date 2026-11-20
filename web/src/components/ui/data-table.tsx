import type { HTMLAttributes, TdHTMLAttributes, ThHTMLAttributes } from "react";

import { cn } from "@/lib/utils";

/**
 * Data table. The wrapper scrolls horizontally inside itself on narrow screens, so the page
 * never gets a horizontal scroll. Always pass `caption` (visually hidden) for screen readers.
 * Compose with `Th` / `Td`; wrap row actions in a `<Td className="text-right">`.
 * The scrolling wrapper is a labelled, focusable region so keyboard users can scroll it too
 * (axe `scrollable-region-focusable`; the global `:focus-visible` ring marks it).
 *
 * (P5-REFINE-2, D-405) `stackBelow` (900 or 640 px) turns the table into stacked rows below that width instead of
 * scrolling: cells say what they are with `<Td cell="title">`, `<Td cell="actions">` and `label="Joined"` (see
 * `ResponsiveTable`, which builds the whole table from data).
 */
export function DataTable({
  caption,
  className,
  children,
  stackBelow,
  ...props
}: { caption: string; stackBelow?: 640 | 900 } & HTMLAttributes<HTMLTableElement>) {
  return (
    <div
      role="region"
      aria-label={caption}
      tabIndex={0}
      className={cn(
        "relative max-w-full rounded-card border border-line bg-surface shadow-card",
        stackBelow === 900 ? "min-[900px]:overflow-x-auto" : stackBelow === 640 ? "sm:overflow-x-auto" : "overflow-x-auto",
      )}
    >
      <table
        data-stack={stackBelow}
        className={cn("w-full text-left text-sm text-ink", stackBelow ? "min-[900px]:min-w-max" : "min-w-max", className)}
        {...props}
      >
        <caption className="sr-only">{caption}</caption>
        {children}
      </table>
    </div>
  );
}

export function Th({ className, ...props }: ThHTMLAttributes<HTMLTableCellElement>) {
  return (
    <th
      scope="col"
      className={cn("whitespace-nowrap bg-canvas px-4 py-2.5 text-xs font-semibold uppercase tracking-label text-muted", className)}
      {...props}
    />
  );
}

/** `cell` is the cell's place in a stacked row ("title" first line, "actions" right edge); `label` is shown before the value there. */
export function Td({
  className,
  cell,
  label,
  ...props
}: { cell?: "title" | "actions"; label?: string } & TdHTMLAttributes<HTMLTableCellElement>) {
  return <td data-cell={cell} data-label={label} className={cn("border-t border-line px-4 py-2.5 align-top", className)} {...props} />;
}
