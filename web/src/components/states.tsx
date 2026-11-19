"use client";

import type { ReactNode } from "react";

import { ApiError } from "@/lib/api/client";
import { errorMessage } from "@/lib/api/errors";
import { copyLabel, useCopy } from "@/lib/use-copy";
import { onlySentence, whoCanOf, type WhoCanValue } from "@/lib/who-can";

import { Button } from "./ui/button";
import { ScrollIcon } from "./ui/icons";
import { WhoCan } from "./ui/who-can";

/**
 * (FIX-146, C-732 (1), docs/14) The failed request's id as a copyable "Reference" for support: the first 8
 * characters with a copy button for the whole id. Nothing for an error without one (network failure, no header).
 */
export function RequestReference({ error }: { error: unknown }) {
  const { state, copy } = useCopy();
  const id = error instanceof ApiError ? error.requestId : undefined;
  if (!id) return null;
  return (
    <p className="mt-2 flex flex-wrap items-center gap-2 text-xs text-ink-2">
      <span>
        Reference: <code className="font-mono">{id.slice(0, 8)}…</code>
      </span>
      <Button
        variant="ghost"
        size="sm"
        className="min-h-8 px-2 text-xs"
        aria-label={`Copy reference ${id}`}
        onClick={() => copy(id)}
      >
        {copyLabel(state)}
      </Button>
    </p>
  );
}

export function Loading({ label = "Loading…" }: { label?: string }) {
  return (
    <p role="status" aria-live="polite" className="flex items-center gap-2 text-sm text-muted">
      <span
        aria-hidden="true"
        className="size-3.5 animate-spin rounded-full border-2 border-line-strong border-t-accent motion-reduce:animate-none"
      />
      {label}
    </p>
  );
}

export function ErrorState({ error, onRetry }: { error: unknown; onRetry?: () => void }) {
  // (C-915) A refusal that names who can fix it reads "Only <role phrase> can open this page." and then who to ask.
  const refused = isRefusal(error) ? whoCanOf(error) : null;
  return (
    <div role="alert" className="rounded-card border border-danger-ink/30 bg-danger-soft p-4 text-sm text-danger-ink">
      <p>{refused ? onlySentence(refused) : errorMessage(error)}</p>
      {refused ? <WhoCan value={refused} className="mt-1 text-danger-ink" /> : null}
      <RequestReference error={error} />
      {onRetry ? (
        <Button variant="outline" size="sm" className="mt-2" onClick={onRetry}>
          Try again
        </Button>
      ) : null}
    </div>
  );
}

/** A 403, or a page gate's `forbidden` (C-915). */
function isRefusal(error: unknown): boolean {
  return typeof error === "object" && error !== null && (error as { code?: unknown }).code === "forbidden";
}

/**
 * An empty list. A *teaching* empty state (D-141) says what the list is (`children`), when you need it (`when`) and the
 * primary action (`action`). A person who cannot act sees who can instead (`ask`, C-916): the sentence is `WhoCan`'s,
 * shown only when `canAct` is false and never next to the action. `ask` cannot be given without `canAct`, so a caller
 * cannot ask someone else on behalf of a viewer who can do it.
 */
export function EmptyState({
  title,
  children,
  when,
  action,
  ask,
  icon,
}: {
  title: string;
  children?: ReactNode;
  when?: ReactNode;
  action?: ReactNode;
  /** `onAsk` adds the **Ask** button next to the who-can line (P5-POLISH-2, C6). */
  ask?: { whoCan: WhoCanValue; canAct: boolean; onAsk?: () => void; /** (P5-APIGAP-2) The whole ask control (**Ask <Name>** / Asked · Undo) instead of the plain button. */ control?: ReactNode };
  /** The icon in the tile; a neutral one by default. */
  icon?: ReactNode;
}) {
  return (
    <div className="rounded-card border border-dashed border-line-strong bg-surface p-6 sm:p-7">
      <span aria-hidden="true" className="flex size-10 items-center justify-center rounded-control bg-subtle text-ink-2 [&_svg]:size-5">
        {icon ?? <ScrollIcon />}
      </span>
      <h2 className="mt-4 font-display text-base font-semibold text-ink">{title}</h2>
      {children ? <div className="mt-2 text-sm text-ink-2">{children}</div> : null}
      {when ? <div className="mt-1 text-sm text-ink-2">{when}</div> : null}
      {ask && !ask.canAct ? (
        <div className="mt-3 flex flex-wrap items-center gap-3 rounded-control bg-subtle px-3 py-2">
          <WhoCan value={ask.whoCan} lead />
          {ask.control ?? null}
          {!ask.control && ask.onAsk ? (
            <Button type="button" variant="outline" size="sm" onClick={ask.onAsk}>
              Ask
            </Button>
          ) : null}
        </div>
      ) : action ? (
        <div className="mt-4 flex flex-wrap gap-2">{action}</div>
      ) : null}
    </div>
  );
}

/**
 * (P5-POLISH-2, C6) A list with a detail pane that hides the pane, and its "Select a … to edit it." text, while the list
 * is empty: an empty list shows only the `empty` state, full width.
 */
export function MasterDetail({
  count,
  empty,
  list,
  detail,
  className,
}: {
  /** How many rows the list has. */
  count: number;
  empty: ReactNode;
  list: ReactNode;
  detail: ReactNode;
  className?: string;
}) {
  if (count === 0) return <>{empty}</>;
  return (
    <div className={className ?? "grid gap-4 md:grid-cols-[minmax(0,18rem)_minmax(0,1fr)]"}>
      <div className="min-w-0">{list}</div>
      <div className="min-w-0">{detail}</div>
    </div>
  );
}
