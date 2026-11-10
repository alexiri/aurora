import type { ReactNode } from "react";

import { errorMessage } from "@/lib/api/errors";

import { Button } from "./ui/button";

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
  return (
    <div role="alert" className="rounded-card border border-danger-ink/30 bg-danger-soft p-4 text-sm text-danger-ink">
      <p>{errorMessage(error)}</p>
      {onRetry ? (
        <Button variant="outline" size="sm" className="mt-2" onClick={onRetry}>
          Try again
        </Button>
      ) : null}
    </div>
  );
}

/**
 * An empty list. A *teaching* empty state (D-141) says what the list is (`children`), when you need it (`when`) and the
 * primary action (`action`). A person who cannot act sees `ask` instead: who to ask, never a dead button.
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
  ask?: ReactNode;
}) {
  return (
    <div className="rounded-card border border-dashed border-line-strong bg-surface p-8 text-center">
      <h2 className="font-display text-base font-semibold text-ink">{title}</h2>
      {children ? <div className="mt-2 text-sm text-ink-2">{children}</div> : null}
      {when ? <div className="mt-1 text-sm text-ink-2">{when}</div> : null}
      {action ? <div className="mt-4 flex flex-wrap justify-center gap-2">{action}</div> : ask ? <div className="mt-3 text-sm text-ink-2">{ask}</div> : null}
    </div>
  );
}
