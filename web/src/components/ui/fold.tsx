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
  "data-testid": testId,
}: {
  summary: ReactNode;
  children: ReactNode;
  className?: string;
  "data-testid"?: string;
}) {
  return (
    <details className={cn("rounded-card border border-line bg-surface px-5 py-3 text-sm", className)} data-testid={testId}>
      <summary className="cursor-pointer font-semibold text-ink">{summary}</summary>
      {children}
    </details>
  );
}
