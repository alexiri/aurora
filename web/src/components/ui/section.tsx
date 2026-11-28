import type { HTMLAttributes, ReactNode } from "react";

import { cn } from "@/lib/utils";

import { Card } from "./card";

/**
 * Page section: heading, optional one-line description and right-aligned actions,
 * then the content. `card` wraps the content in a Card. `id` links the heading to the
 * region (`aria-labelledby`).
 */
export function Section({
  id,
  title,
  description,
  actions,
  card = false,
  pageTitle = false,
  children,
  className,
}: {
  id: string;
  title: ReactNode;
  description?: ReactNode;
  actions?: ReactNode;
  card?: boolean;
  /** The section is the whole page under a tab row: its heading has a page title's size. */
  pageTitle?: boolean;
  children: ReactNode;
  className?: string;
}) {
  return (
    <section id={id} aria-labelledby={`${id}-h`} className={cn("space-y-3", className)}>
      <div className="flex flex-wrap items-end justify-between gap-x-4 gap-y-2">
        <div className="min-w-0">
          <h2 id={`${id}-h`} className={cn("font-display font-semibold text-ink", pageTitle ? "text-2xl" : "text-base")}>
            {title}
          </h2>
          {description ? <p className="mt-0.5 text-sm text-ink-2">{description}</p> : null}
        </div>
        {actions ? <div className="flex flex-wrap items-center gap-2">{actions}</div> : null}
      </div>
      {card ? <Card>{children}</Card> : children}
    </section>
  );
}

/**
 * Destructive actions (delete, offboard, revoke everything): a Card with a danger border, a
 * `danger-ink` heading, an explanation and the action as children. Use it at the bottom of a page.
 */
export function DangerZone({
  id,
  title,
  description,
  children,
  className,
}: {
  id: string;
  title: ReactNode;
  description?: ReactNode;
  children: ReactNode;
  className?: string;
}) {
  return (
    <section aria-labelledby={`${id}-h`} className={cn("space-y-3 rounded-card border border-danger-ink/40 bg-surface p-4 shadow-card sm:p-5", className)}>
      <h2 id={`${id}-h`} className="font-display text-base font-semibold text-danger-ink">
        {title}
      </h2>
      {description ? <p className="text-sm text-ink-2">{description}</p> : null}
      {children}
    </section>
  );
}

/** Divided list in a bordered surface; rows are `<ListRow>`. Give it an `aria-label`. */
export function ItemList({ className, ...props }: HTMLAttributes<HTMLUListElement>) {
  return (
    <ul
      className={cn("divide-y divide-line overflow-hidden rounded-card border border-line bg-surface shadow-card", className)}
      {...props}
    />
  );
}

/**
 * One list row: wraps on narrow screens; `actions` stay on the right (or below when tight). `wideText`: a row of running
 * text beside several buttons; the text keeps at least 16 rem, and the buttons go below it when that doesn't fit.
 */
export function ListRow({
  children,
  actions,
  className,
  wideText = false,
}: {
  children: ReactNode;
  actions?: ReactNode;
  className?: string;
  wideText?: boolean;
}) {
  return (
    <li className={cn("flex flex-wrap items-center gap-x-3 gap-y-2 p-3 sm:px-4", className)}>
      <div className={cn("flex min-w-0 flex-wrap items-center gap-x-3 gap-y-1", wideText ? "grow basis-64" : "flex-1")}>{children}</div>
      {actions ? <div className="flex shrink-0 flex-wrap items-center gap-2">{actions}</div> : null}
    </li>
  );
}
