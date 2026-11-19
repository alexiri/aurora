import Link from "next/link";
import type { ReactNode } from "react";

import { cn } from "@/lib/utils";

import { ArrowRightIcon } from "./icons";

/** (P5-POLISH-2) A card that is a link: a title, one line, a trailing arrow, hover and focus states. */
export function LinkCard({
  href,
  title,
  children,
  className,
}: {
  href: string;
  title: string;
  children?: ReactNode;
  className?: string;
}) {
  return (
    <Link
      href={href}
      className={cn(
        "group flex items-center justify-between gap-3 rounded-card border border-line bg-surface p-4 text-ink shadow-card hover:border-line-strong hover:bg-subtle sm:p-5",
        className,
      )}
    >
      <span className="min-w-0">
        <span className="block font-display text-base font-semibold">{title}</span>
        {children ? <span className="mt-0.5 block text-sm text-ink-2">{children}</span> : null}
      </span>
      <ArrowRightIcon aria-hidden="true" className="size-5 shrink-0 text-ink-2 transition-transform group-hover:translate-x-0.5 motion-reduce:transition-none" />
    </Link>
  );
}
