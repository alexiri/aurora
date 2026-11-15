"use client";

import { useId } from "react";

import { Badge } from "@/components/ui/badge";
import { cn } from "@/lib/utils";

export interface ChoiceCardOption<T extends string> {
  value: T;
  label: string;
  /** One or two sentences: what the choice is and when to use it. */
  description?: string;
  /** A short pill next to the label, for example "Recommended". */
  badge?: string;
}

/**
 * A single choice among a few options that need explaining, shown as stacked cards (native radio inputs, so arrow keys
 * and form semantics come for free). Each card carries its label, an optional badge and a description; the chosen
 * card is outlined and tinted, and the choice never depends on colour alone (the radio keeps its checked state for
 * assistive technology and the badge is text). `legend` names the group.
 */
export function ChoiceCards<T extends string>({
  legend,
  value,
  options,
  onChange,
  disabled = false,
}: {
  legend: string;
  value: T;
  options: ChoiceCardOption<T>[];
  onChange: (v: T) => void;
  disabled?: boolean;
}) {
  const name = useId();
  const legendId = `${name}-legend`;
  return (
    <div role="radiogroup" aria-labelledby={legendId} className="min-w-0 space-y-2">
      <p id={legendId} className="text-sm font-medium text-ink">
        {legend}
      </p>
      {options.map((o) => (
        <label key={o.value} className={cn("relative block", disabled && "opacity-60")}>
          <input
            type="radio"
            name={name}
            value={o.value}
            checked={value === o.value}
            disabled={disabled}
            onChange={() => onChange(o.value)}
            className="peer sr-only"
          />
          <span className="block min-h-11 cursor-pointer rounded-control border border-line bg-surface p-3 hover:border-line-strong peer-checked:border-accent peer-checked:bg-accent-soft peer-focus-visible:outline peer-focus-visible:outline-2 peer-focus-visible:outline-accent dark:bg-canvas dark:peer-checked:bg-accent-soft">
            <span className="flex flex-wrap items-center gap-2">
              <span className="text-sm font-semibold text-ink">{o.label}</span>
              {o.badge ? <Badge tone="accent">{o.badge}</Badge> : null}
            </span>
            {o.description ? <span className="mt-1 block text-sm text-ink-2">{o.description}</span> : null}
          </span>
        </label>
      ))}
    </div>
  );
}
