import type { ReactNode } from "react";

import { cn } from "@/lib/utils";

/**
 * The grid every list of cards uses (D-439, docs/14 §Visual design): Apps, Accounts, Agents. The columns follow the
 * width of the content area, not the viewport (the sidebar takes 256 px of it): one column below 672 px, two from
 * `@2xl`, three from `@4xl` (896 px), four from `@7xl` (1280 px). Its children are `<li>` cards; a card fills its
 * cell (`h-full`) and puts its actions at the foot (`mt-auto`).
 */
export function CardGrid({
  children,
  className,
  ...rest
}: {
  children: ReactNode;
  className?: string;
  "aria-label"?: string;
  "aria-hidden"?: boolean | "true" | "false";
}) {
  return (
    <div className="@container">
      <ul {...rest} className={cn("grid gap-4 @2xl:grid-cols-2 @4xl:grid-cols-3 @7xl:grid-cols-4", className)}>
        {children}
      </ul>
    </div>
  );
}

/** Skeleton cards in the grid's own shape while a list of cards loads (Apps, Accounts). */
export function CardGridSkeleton({ label, count = 6 }: { label: string; count?: number }) {
  return (
    <div role="status" aria-live="polite" aria-busy="true">
      <span className="sr-only">{label}</span>
      <CardGrid aria-hidden="true">
        {Array.from({ length: count }, (_, i) => (
          <li key={i} data-testid="skeleton-card" className="flex flex-col gap-3 rounded-card border border-line bg-surface p-4 shadow-card sm:p-5">
            <div className="flex items-center gap-3">
              <span className="size-9 animate-pulse rounded-control bg-subtle motion-reduce:animate-none" />
              <span className="h-4 w-1/3 animate-pulse rounded bg-subtle motion-reduce:animate-none" />
            </div>
            <span className="h-3 w-full animate-pulse rounded bg-subtle motion-reduce:animate-none" />
            <span className="h-3 w-2/3 animate-pulse rounded bg-subtle motion-reduce:animate-none" />
          </li>
        ))}
      </CardGrid>
      <p className="mt-3 text-xs text-muted">{label}</p>
    </div>
  );
}
