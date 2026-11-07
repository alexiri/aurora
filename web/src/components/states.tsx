import type { ReactNode } from "react";

import { errorMessage } from "@/lib/api/errors";

import { Button } from "./ui/button";

export function Loading({ label = "Loading…" }: { label?: string }) {
  return (
    <p role="status" aria-live="polite" className="text-sm text-neutral-500">
      {label}
    </p>
  );
}

export function ErrorState({ error, onRetry }: { error: unknown; onRetry?: () => void }) {
  return (
    <div role="alert" className="rounded-md border border-red-200 bg-red-50 p-4 text-sm text-red-800">
      <p>{errorMessage(error)}</p>
      {onRetry ? (
        <Button variant="outline" size="sm" className="mt-2" onClick={onRetry}>
          Try again
        </Button>
      ) : null}
    </div>
  );
}

export function EmptyState({
  title,
  children,
}: {
  title: string;
  children?: ReactNode;
}) {
  return (
    <div className="rounded-md border border-dashed border-neutral-300 p-8 text-center">
      <h2 className="font-medium">{title}</h2>
      {children ? <div className="mt-2 text-sm text-neutral-600">{children}</div> : null}
    </div>
  );
}
