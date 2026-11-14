import type { HTMLAttributes, TdHTMLAttributes, ThHTMLAttributes } from "react";

import { cn } from "@/lib/utils";

/**
 * Data table. The wrapper scrolls horizontally inside itself on narrow screens, so the page
 * never gets a horizontal scroll. Always pass `caption` (visually hidden) for screen readers.
 * Compose with `Th` / `Td`; wrap row actions in a `<Td className="text-right">`.
 */
export function DataTable({
  caption,
  className,
  children,
  ...props
}: { caption: string } & HTMLAttributes<HTMLTableElement>) {
  return (
    // A region that scrolls must be reachable by keyboard (axe `scrollable-region-focusable`).
    <div
      role="region"
      aria-label={caption}
      tabIndex={0}
      className="relative max-w-full overflow-x-auto rounded-card border border-line bg-surface shadow-card focus-visible:outline focus-visible:outline-2 focus-visible:outline-accent"
    >
      <table className={cn("w-full min-w-max text-left text-sm text-ink", className)} {...props}>
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

export function Td({ className, ...props }: TdHTMLAttributes<HTMLTableCellElement>) {
  return <td className={cn("border-t border-line px-4 py-2.5 align-top", className)} {...props} />;
}
