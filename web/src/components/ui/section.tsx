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
  children,
  className,
}: {
  id: string;
  title: ReactNode;
  description?: ReactNode;
  actions?: ReactNode;
  card?: boolean;
  children: ReactNode;
  className?: string;
}) {
  return (
    <section aria-labelledby={`${id}-h`} className={cn("space-y-3", className)}>
      <div className="flex flex-wrap items-end justify-between gap-x-4 gap-y-2">
        <div className="min-w-0">
          <h2 id={`${id}-h`} className="font-display text-base font-semibold text-ink">
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

/** One list row: wraps on narrow screens; `actions` stay on the right (or below when tight). */
export function ListRow({
  children,
  actions,
  className,
}: {
  children: ReactNode;
  actions?: ReactNode;
  className?: string;
}) {
  return (
    <li className={cn("flex flex-wrap items-center gap-x-3 gap-y-2 p-3 sm:px-4", className)}>
      <div className="flex min-w-0 flex-1 flex-wrap items-center gap-x-3 gap-y-1">{children}</div>
      {actions ? <div className="flex shrink-0 flex-wrap items-center gap-2">{actions}</div> : null}
    </li>
  );
}
