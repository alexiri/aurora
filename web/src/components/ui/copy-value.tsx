"use client";

import { useId } from "react";

import { useCopy } from "@/lib/use-copy";
import { cn } from "@/lib/utils";

import { Button } from "./button";
import { CheckIcon, CopyIcon } from "./icons";

/**
 * (P5-MEMORY-2) A short identifier shown inline as a small monospace chip with its copy button (a memory's key): the
 * visible `label` before it, the button named "Copy <label>". The tick replaces the icon for two seconds.
 */
export function CopyChip({ label, value, className }: { label: string; value: string; className?: string }) {
  const { state, copy } = useCopy();
  return (
    <span className={cn("inline-flex max-w-full items-center gap-2 text-xs text-muted", className)}>
      <span aria-hidden="true">{label}</span>
      <span className="inline-flex min-w-0 items-center gap-1 rounded-control bg-subtle pl-2">
        <span className="min-w-0 select-all break-all font-mono text-ink" data-testid="copy-chip-text">
          {value}
        </span>
        <button
          type="button"
          aria-label={`Copy ${label}`}
          onClick={() => copy(value)}
          className="inline-flex size-8 shrink-0 items-center justify-center rounded-control text-ink-2 hover:text-ink"
        >
          {state === "copied" ? <CheckIcon className="size-3.5" /> : <CopyIcon className="size-3.5" />}
        </button>
      </span>
      <span role="status" className={state === "failed" ? "text-danger-ink" : "sr-only"}>
        {state === "copied" ? "Copied" : state === "failed" ? "Your browser blocked copying; select the text instead." : ""}
      </span>
    </span>
  );
}

/**
 * A value the person copies into another service (an OAuth callback URL, the organization's external id): a label,
 * the value in monospace, and a Copy button (P5-UX-13, C-942; the Patterns board's `.copy` row). The value wraps inside
 * the row (no horizontal scroll at 390 px) and is selectable; the button says "Copied" for two seconds, and a browser
 * that refuses says so. `hint` is one sentence under the row ("not a secret").
 */
export function CopyValue({
  label,
  value,
  hint,
  className,
}: {
  label: string;
  value: string;
  hint?: string;
  className?: string;
}) {
  const id = useId();
  const { state, copy } = useCopy();
  return (
    <div className={cn("space-y-1.5", className)} data-testid="copy-value">
      <p id={`${id}-label`} className="text-sm font-medium text-ink">
        {label}
      </p>
      <div className="flex min-w-0 items-center gap-2.5 rounded-control border border-line-strong bg-canvas px-2.5 py-2">
        <span
          aria-labelledby={`${id}-label`}
          className="min-w-0 flex-1 select-all break-all font-mono text-xs text-ink"
          data-testid="copy-value-text"
        >
          {value}
        </span>
        <Button
          type="button"
          variant="outline"
          size="sm"
          aria-label={`Copy ${label}`}
          onClick={() => copy(value)}
        >
          {state === "copied" ? "Copied" : "Copy"}
        </Button>
      </div>
      {state === "failed" ? (
        <p role="status" className="text-xs text-danger-ink">
          Your browser blocked copying; select the text instead.
        </p>
      ) : state === "copied" ? (
        <p role="status" className="sr-only">
          Copied
        </p>
      ) : null}
      {hint ? <p className="text-xs text-muted">{hint}</p> : null}
    </div>
  );
}
