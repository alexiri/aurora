"use client";

import Link from "next/link";
import { createContext, useContext, type ReactNode } from "react";

import { cn } from "@/lib/utils";

import { Badge } from "./badge";

export interface Crumb {
  label: string;
  href?: string;
}

/** Where a page lives (C-962): Administration, an organization, or a workspace of one. */
export interface PageLevel {
  kind: "instance" | "org" | "workspace";
  label: string;
}

const LevelContext = createContext<PageLevel | null>(null);
/** The app shell provides the level of the page in the URL, so every header carries its badge without a prop. */
export const PageLevelProvider = LevelContext.Provider;

type Embedded = "off" | "hidden" | "headed";
const EmbeddedContext = createContext<Embedded>("off");
/**
 * A topic page hosts the content of the pages it replaces. Their own headers must not add a second title:
 * inside this provider a `PageHeader` keeps its description and actions and drops the title, breadcrumb and badge.
 * With `headed` (several hosted pages stacked in one tab) each keeps its title, as a section heading.
 */
export function EmbeddedPage({ children, headed = false }: { children: ReactNode; headed?: boolean }) {
  return <EmbeddedContext.Provider value={headed ? "headed" : "hidden"}>{children}</EmbeddedContext.Provider>;
}

export function LevelBadge({ level, className }: { level: PageLevel; className?: string }) {
  return (
    <Badge
      tone={level.kind === "instance" ? "pending" : "neutral"}
      data-testid="level-badge"
      data-level={level.kind}
      className={cn("max-w-full normal-case tracking-normal", className)}
    >
      <span className="truncate">{level.label}</span>
    </Badge>
  );
}

/** Breadcrumb, title, one-line description and right-aligned actions (wrapping on narrow screens). */
export function PageHeader({
  title,
  description,
  breadcrumb,
  actions,
  className,
  compact = false,
  level,
}: {
  title: ReactNode;
  description?: ReactNode;
  breadcrumb?: Crumb[];
  actions?: ReactNode;
  className?: string;
  /** One-line variant for pages that must fit the viewport: the description sits on the title line. */
  compact?: boolean;
  /** The level badge; defaults to the level of the page in the URL (the shell provides it). */
  level?: PageLevel;
}) {
  const inherited = useContext(LevelContext);
  const embedded = useContext(EmbeddedContext);
  const shown = level ?? inherited;
  if (embedded !== "off") {
    const headed = embedded === "headed";
    if (!headed && !description && !actions) return null;
    return (
      <div className={cn("mb-3 flex flex-wrap items-start justify-between gap-x-4 gap-y-2", className)}>
        <div className="min-w-0">
          {headed ? <h2 className="font-display text-xl font-semibold leading-7 text-ink">{title}</h2> : null}
          {description ? <div className="text-sm text-ink-2">{description}</div> : null}
        </div>
        {actions ? <div className="flex flex-wrap items-center gap-2">{actions}</div> : null}
      </div>
    );
  }
  const crumbs =
    breadcrumb && breadcrumb.length > 0 ? (
      <nav aria-label="Breadcrumb" className={cn("text-xs text-muted", compact ? "mb-1" : "mb-2")}>
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
    ) : null;
  const badge = shown ? <LevelBadge level={shown} className="mb-1.5" /> : null;
  if (compact) {
    return (
      <header className={cn("mb-2", className)}>
        {crumbs}
        <div className="flex flex-wrap items-center justify-between gap-x-4 gap-y-2">
          <div className="flex min-w-0 flex-wrap items-center gap-x-3 gap-y-1">
            <h1 className="min-w-0 font-display text-xl font-semibold leading-7 text-ink">{title}</h1>
            {shown ? <LevelBadge level={shown} /> : null}
            {description ? <div className="text-sm text-ink-2">{description}</div> : null}
          </div>
          {actions ? <div className="flex flex-wrap items-center gap-2">{actions}</div> : null}
        </div>
      </header>
    );
  }
  return (
    <header className={cn("mb-6", className)}>
      {crumbs}
      <div className="flex flex-wrap items-start justify-between gap-x-4 gap-y-3">
        <div className="min-w-0">
          {badge}
          <h1 className="font-display text-3xl font-semibold leading-9 text-ink">{title}</h1>
          {description ? <p className="mt-1 text-sm text-ink-2">{description}</p> : null}
        </div>
        {actions ? <div className="flex flex-wrap items-center gap-2">{actions}</div> : null}
      </div>
    </header>
  );
}
