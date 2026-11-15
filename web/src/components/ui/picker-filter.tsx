"use client";

import { Field, inputClass } from "./field";
import { cn } from "@/lib/utils";

/** A picker with more options than this gets a text filter (docs/14: "add by name or e-mail"). */
export const PICKER_FILTER_FROM = 8;

/** Case-insensitive "contains" over any of the given texts; an empty query matches everything. */
export function matchesFilter(query: string, ...texts: (string | null | undefined)[]): boolean {
  const q = query.trim().toLowerCase();
  return q === "" || texts.some((t) => (t ?? "").toLowerCase().includes(q));
}

/**
 * The text filter that sits beside a long `<select>` picker. Shown only when the list is long
 * (`count > PICKER_FILTER_FROM`), so a short picker stays as it was.
 */
export function PickerFilter({
  id,
  label,
  count,
  value,
  onChange,
  className,
}: {
  id: string;
  label: string;
  count: number;
  value: string;
  onChange: (v: string) => void;
  className?: string;
}) {
  if (count <= PICKER_FILTER_FROM && value === "") return null;
  return (
    <Field id={id} label={label}>
      <input
        id={id}
        type="search"
        className={cn(inputClass, "w-56 max-w-full", className)}
        value={value}
        autoComplete="off"
        onChange={(e) => onChange(e.target.value)}
      />
    </Field>
  );
}
