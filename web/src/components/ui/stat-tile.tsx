import Link from "next/link";
import type { ReactNode } from "react";

import { cn } from "@/lib/utils";

/** (D-508, nav-3b) A summary card that is a link: what is counted, and how many, in large figures. */
export function StatLink({ href, label, value }: { href: string; label: string; value: ReactNode }) {
  return (
    <Link href={href} className="block min-w-0 rounded-card border border-line bg-surface p-4 shadow-card hover:border-line-strong hover:bg-subtle">
      <span className="block text-xs text-muted">{label}</span>{" "}
      <span className="mt-1 block font-display text-2xl font-semibold tabular-nums text-ink">{value}</span>
    </Link>
  );
}

/**
 * (C-1005) One figure: a label in `muted`, the value in `ink` with tabular figures, an optional detail line in
 * `ink-2`. `title` carries the exact value for a compact one (tooltip on hover).
 */
export function StatTile({
  label,
  value,
  detail,
  title,
  className,
}: {
  label: string;
  value: ReactNode;
  detail?: ReactNode;
  title?: string;
  className?: string;
}) {
  return (
    <div className={cn("min-w-0 rounded-card border border-line bg-surface p-4 shadow-card", className)}>
      <div className="text-xs font-semibold uppercase tracking-label text-muted">{label}</div>
      <div className="mt-1 font-display text-2xl font-semibold tabular-nums text-ink" title={title}>
        {value}
      </div>
      {detail ? <div className="mt-1 text-xs text-ink-2">{detail}</div> : null}
    </div>
  );
}
