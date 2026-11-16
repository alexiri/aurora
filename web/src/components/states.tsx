"use client";

import { useState, type ReactNode } from "react";

import { ApiError } from "@/lib/api/client";
import { errorMessage } from "@/lib/api/errors";
import { onlySentence, whoCanOf, type WhoCanValue } from "@/lib/who-can";

import { Button } from "./ui/button";
import { WhoCan } from "./ui/who-can";

/**
 * (FIX-146, C-732 (1), docs/14) The failed request's id as a copyable "Reference" for support: the first 8
 * characters with a copy button for the whole id. Nothing for an error without one (network failure, no header).
 */
export function RequestReference({ error }: { error: unknown }) {
  const [copied, setCopied] = useState(false);
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
        onClick={() => {
          void navigator.clipboard?.writeText(id).then(() => setCopied(true));
        }}
      >
        {copied ? "Copied" : "Copy"}
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
}: {
  title: string;
  children?: ReactNode;
  when?: ReactNode;
  action?: ReactNode;
  ask?: { whoCan: WhoCanValue; canAct: boolean };
}) {
  return (
    <div className="rounded-card border border-dashed border-line-strong bg-surface p-8 text-center">
      <h2 className="font-display text-base font-semibold text-ink">{title}</h2>
      {children ? <div className="mt-2 text-sm text-ink-2">{children}</div> : null}
      {when ? <div className="mt-1 text-sm text-ink-2">{when}</div> : null}
      {ask && !ask.canAct ? (
        <WhoCan value={ask.whoCan} lead className="mt-3" />
      ) : action ? (
        <div className="mt-4 flex flex-wrap justify-center gap-2">{action}</div>
      ) : null}
    </div>
  );
}
