"use client";

import type { InputHTMLAttributes, ReactNode } from "react";

import { cn } from "@/lib/utils";

/**
 * (P5-POLISH-2, D-392) A switch for a boolean setting: a native checkbox with `role="switch"` (keyboard, form and screen
 * reader behaviour come with it), drawn 40×24 from tokens. Name it with a `<label htmlFor>` (see `SwitchField`) or
 * `aria-label`. True multi-select lists stay checkboxes.
 */
export function Switch({
  checked,
  onCheckedChange,
  className,
  ...props
}: Omit<InputHTMLAttributes<HTMLInputElement>, "type" | "role" | "onChange" | "checked"> & {
  checked: boolean;
  onCheckedChange: (checked: boolean) => void;
}) {
  return (
    <span className={cn("relative inline-flex h-6 w-10 shrink-0 align-middle", className)}>
      <input
        {...props}
        type="checkbox"
        role="switch"
        checked={checked}
        onChange={(e) => onCheckedChange(e.target.checked)}
        className="peer absolute inset-0 z-10 m-0 size-full cursor-pointer appearance-none rounded-full outline-offset-2 disabled:cursor-not-allowed"
      />
      <span
        aria-hidden="true"
        className="pointer-events-none absolute inset-0 rounded-full border border-line-strong bg-subtle transition-colors peer-checked:border-accent peer-checked:bg-accent peer-disabled:opacity-50"
      />
      <span
        aria-hidden="true"
        className="pointer-events-none absolute left-1 top-1 size-4 rounded-full bg-ink-2 transition-transform peer-checked:translate-x-4 peer-checked:bg-on-accent peer-disabled:opacity-50 motion-reduce:transition-none"
      />
    </span>
  );
}

/** A switch with its label on the left and optional help under it: one row for a form. `id` links label and control. */
export function SwitchField({
  id,
  label,
  hint,
  checked,
  onChange,
  disabled,
  className,
}: {
  id: string;
  label: ReactNode;
  hint?: ReactNode;
  checked: boolean;
  onChange: (checked: boolean) => void;
  disabled?: boolean;
  className?: string;
}) {
  return (
    <div className={cn("flex items-start justify-between gap-4", className)}>
      <div className="min-w-0">
        <label htmlFor={id} className="block text-sm font-medium text-ink">
          {label}
        </label>
        {hint ? (
          <p id={`${id}-hint`} className="mt-0.5 text-xs text-muted">
            {hint}
          </p>
        ) : null}
      </div>
      <Switch id={id} checked={checked} onCheckedChange={onChange} disabled={disabled} aria-describedby={hint ? `${id}-hint` : undefined} />
    </div>
  );
}
