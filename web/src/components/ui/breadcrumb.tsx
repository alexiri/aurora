import Link from "next/link";
import { Fragment } from "react";

import { cn } from "@/lib/utils";

export interface BreadcrumbItem {
  label: string;
  /** Where the segment goes; the last item (the current place) has none and is marked `aria-current`. */
  href?: string;
  /** Called on a plain click instead of following `href` (the Files page navigates without a reload). */
  onSelect?: () => void;
}

/**
 * A trail of places (P5-FILES-2, docs/14 §Files): each segment but the last is a link, the last is the current place. The
 * trail wraps onto further lines instead of scrolling, and a long segment breaks anywhere, so it never makes the page
 * scroll sideways at 390 px.
 */
export function Breadcrumb({ label, items, className }: { label: string; items: BreadcrumbItem[]; className?: string }) {
  return (
    <nav aria-label={label} className={className}>
      <ol className="flex min-w-0 flex-wrap items-center gap-x-1 gap-y-0.5 text-sm">
        {items.map((item, i) => {
          const last = i === items.length - 1;
          return (
            <Fragment key={`${i}:${item.href ?? item.label}`}>
              <li className="min-w-0 max-w-full">
                {last || !item.href ? (
                  <span aria-current={last ? "page" : undefined} className={cn("break-all font-medium", last ? "text-ink" : "text-ink-2")}>
                    {item.label}
                  </span>
                ) : (
                  <Link
                    href={item.href}
                    onClick={(e) => {
                      if (!item.onSelect || e.metaKey || e.ctrlKey || e.shiftKey || e.altKey || e.button !== 0) return;
                      e.preventDefault();
                      item.onSelect();
                    }}
                    className="inline-flex min-h-9 items-center break-all rounded-control px-1 text-ink-2 hover:text-ink hover:underline md:min-h-7">
                    {item.label}
                  </Link>
                )}
              </li>
              {last ? null : (
                <li aria-hidden="true" className="text-muted">
                  /
                </li>
              )}
            </Fragment>
          );
        })}
      </ol>
    </nav>
  );
}
