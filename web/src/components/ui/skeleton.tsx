import { cn } from "@/lib/utils";

/**
 * (P5-REFINE-2, workspace-12) Placeholder rows while a list loads: the same height as a list row, so the page does not
 * jump when the rows arrive. The pulse is off under `prefers-reduced-motion`. A skeleton hides the wait, it does not
 * remove it. Announced once as a polite status ("Loading…").
 */
export function SkeletonRows({ rows = 4, label = "Loading…", className }: { rows?: number; label?: string; className?: string }) {
  return (
    <div role="status" aria-live="polite" aria-busy="true" className={cn("divide-y divide-line overflow-hidden rounded-card border border-line bg-surface", className)}>
      <span className="sr-only">{label}</span>
      {Array.from({ length: rows }, (_, i) => (
        <div key={i} aria-hidden="true" data-testid="skeleton-row" className="flex items-center gap-3 px-4 py-3">
          <span className="size-8 shrink-0 animate-pulse rounded-control bg-subtle motion-reduce:animate-none" />
          <span className="flex min-w-0 flex-1 flex-col gap-2">
            <span className="h-3 w-1/3 animate-pulse rounded bg-subtle motion-reduce:animate-none" />
            <span className="h-3 w-2/3 animate-pulse rounded bg-subtle motion-reduce:animate-none" />
          </span>
        </div>
      ))}
    </div>
  );
}
