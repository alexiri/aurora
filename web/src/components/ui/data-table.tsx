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
    <div className="max-w-full overflow-x-auto rounded-card border border-line bg-surface shadow-card">
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
      className={cn("whitespace-nowrap bg-canvas px-4 py-2.5 text-xs font-semibold uppercase tracking-[0.06em] text-muted", className)}
      {...props}
    />
  );
}

export function Td({ className, ...props }: TdHTMLAttributes<HTMLTableCellElement>) {
  return <td className={cn("border-t border-line px-4 py-2.5 align-top", className)} {...props} />;
}
