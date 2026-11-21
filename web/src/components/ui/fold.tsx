import type { ReactNode } from "react";

import { cn } from "@/lib/utils";

/**
 * A closed fold: one line that opens to more (Skills' "How skills combine", Approvals' "What an approval looks like").
 * Closed on every visit; the summary is its name. `className` sets the spacing around it.
 */
export function Fold({
  summary,
  children,
  className,
  defaultOpen,
  "data-testid": testId,
}: {
  summary: ReactNode;
  children: ReactNode;
  className?: string;
  /** Open when it first appears (a fold that holds something already filled in); closed otherwise. */
  defaultOpen?: boolean;
  "data-testid"?: string;
}) {
  return (
    <details open={defaultOpen || undefined} className={cn("rounded-card border border-line bg-surface px-5 py-3 text-sm", className)} data-testid={testId}>
      <summary className="cursor-pointer font-semibold text-ink">{summary}</summary>
      {children}
    </details>
  );
}
