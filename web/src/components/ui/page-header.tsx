import Link from "next/link";
import type { ReactNode } from "react";

import { cn } from "@/lib/utils";

export interface Crumb {
  label: string;
  href?: string;
}

/** Breadcrumb, title, one-line description and right-aligned actions (wrapping on narrow screens). */
export function PageHeader({
  title,
  description,
  breadcrumb,
  actions,
  className,
}: {
  title: ReactNode;
  description?: ReactNode;
  breadcrumb?: Crumb[];
  actions?: ReactNode;
  className?: string;
}) {
  return (
    <header className={cn("mb-6", className)}>
      {breadcrumb && breadcrumb.length > 0 ? (
        <nav aria-label="Breadcrumb" className="mb-2 text-xs text-muted">
          <ol className="flex flex-wrap items-center gap-1">
            {breadcrumb.map((c, i) => (
              <li key={`${c.label}-${i}`} className="flex items-center gap-1">
                {i > 0 ? <span aria-hidden="true">/</span> : null}
                {c.href ? (
                  <Link href={c.href} className="hover:text-ink hover:underline">
                    {c.label}
                  </Link>
                ) : (
                  <span aria-current="page">{c.label}</span>
                )}
              </li>
            ))}
          </ol>
        </nav>
      ) : null}
      <div className="flex flex-wrap items-start justify-between gap-x-4 gap-y-3">
        <div className="min-w-0">
          <h1 className="font-display text-[30px] font-semibold leading-9 text-ink">{title}</h1>
          {description ? <p className="mt-1 text-sm text-ink-2">{description}</p> : null}
        </div>
        {actions ? <div className="flex flex-wrap items-center gap-2">{actions}</div> : null}
      </div>
    </header>
  );
}
