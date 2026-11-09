"use client";

import { useId } from "react";

import { cn } from "@/lib/utils";

export interface SegmentedOption<T extends string> {
  value: T;
  label: string;
}

/**
 * A small single-choice control (native radio inputs, so arrow keys and form semantics come for free), for a few
 * short options such as Allow / Ask / Block. `label` names the group for assistive technology.
 */
export function Segmented<T extends string>({
  label,
  value,
  options,
  onChange,
  disabled = false,
  className,
}: {
  label: string;
  /** `null` = none chosen (for example a group whose rows differ). */
  value: T | null;
  options: SegmentedOption<T>[];
  onChange: (v: T) => void;
  disabled?: boolean;
  className?: string;
}) {
  const name = useId();
  return (
    <div role="radiogroup" aria-label={label} className={cn("inline-flex max-w-full gap-0.5 rounded-control bg-subtle p-0.5", className)}>
      {options.map((o) => (
        <label key={o.value} className={cn("relative", disabled && "opacity-60")}>
          <input
            type="radio"
            name={name}
            value={o.value}
            checked={value === o.value}
            disabled={disabled}
            onChange={() => onChange(o.value)}
            className="peer sr-only"
          />
          <span className="inline-flex min-h-9 cursor-pointer items-center whitespace-nowrap rounded-md px-3 text-sm font-medium text-ink-2 hover:text-ink peer-checked:bg-surface peer-checked:font-semibold peer-checked:text-ink peer-checked:shadow-card peer-focus-visible:outline peer-focus-visible:outline-2 peer-focus-visible:outline-accent">
            {o.label}
          </span>
        </label>
      ))}
    </div>
  );
}
