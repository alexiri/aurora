"use client";

import { cn } from "@/lib/utils";

export interface ChoiceChipOption<T extends string> {
  value: T;
  label: string;
}

/**
 * (P5-REFINE-12) A row of choice chips: one choice among a handful of short labels (a purpose to start from). Native
 * buttons with `role="radio"`, wrapping on narrow screens. `label` names the group; the chosen chip is outlined, tinted
 * and `aria-checked`, so the choice never depends on colour alone.
 */
export function ChoiceChips<T extends string>({
  label,
  value,
  options,
  onChange,
  className,
}: {
  label: string;
  value: T;
  options: readonly ChoiceChipOption<T>[];
  onChange: (v: T) => void;
  className?: string;
}) {
  return (
    <div role="radiogroup" aria-label={label} className={cn("flex min-w-0 flex-wrap gap-2", className)}>
      {options.map((o) => (
        <button
          key={o.value}
          type="button"
          role="radio"
          aria-checked={value === o.value}
          onClick={() => onChange(o.value)}
          className={cn(
            "min-h-9 rounded-full border px-3 py-1 text-sm font-medium focus-visible:outline focus-visible:outline-2 focus-visible:outline-accent",
            value === o.value ? "border-accent bg-accent-soft text-ink" : "border-line bg-surface text-ink-2 hover:border-line-strong dark:bg-canvas",
          )}
        >
          {o.label}
        </button>
      ))}
    </div>
  );
}
