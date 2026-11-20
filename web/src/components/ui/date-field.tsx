"use client";

import { useMemo, useRef, useState } from "react";

import { CalendarIcon } from "@/components/ui/icons";
import { cn } from "@/lib/utils";

import { inputClass } from "./field";

type Part = "day" | "month" | "year";

/** The order of day, month and year in the browser's locale, and its separator ("19/11/2026" → day, month, year, "/"). */
export function localeDatePattern(locale?: string): { order: Part[]; sep: string } {
  const parts = new Intl.DateTimeFormat(locale, { year: "numeric", month: "2-digit", day: "2-digit", timeZone: "UTC" }).formatToParts(
    new Date(Date.UTC(2026, 10, 19)),
  );
  const order = parts.filter((p) => p.type === "day" || p.type === "month" || p.type === "year").map((p) => p.type as Part);
  const sep = parts.find((p) => p.type === "literal")?.value.trim() || "/";
  return { order, sep };
}

/** `YYYY-MM-DD` → the locale's text ("19/11/2026"), "" for nothing. */
export function isoToLocaleText(iso: string, locale?: string): string {
  const m = /^(\d{4})-(\d{2})-(\d{2})$/.exec(iso);
  if (!m) return "";
  return new Intl.DateTimeFormat(locale, { year: "numeric", month: "2-digit", day: "2-digit", timeZone: "UTC" }).format(
    new Date(Date.UTC(Number(m[1]), Number(m[2]) - 1, Number(m[3]))),
  );
}

/** The locale's text → `YYYY-MM-DD`, or `null` when it is not a real date. */
export function localeTextToIso(text: string, locale?: string): string | null {
  const nums = text.trim().split(/\D+/).filter(Boolean);
  if (nums.length !== 3) return null;
  const { order } = localeDatePattern(locale);
  const v: Record<Part, number> = { day: 0, month: 0, year: 0 };
  order.forEach((p, i) => {
    v[p] = Number(nums[i]);
  });
  if (v.year < 100) v.year += 2000;
  const d = new Date(Date.UTC(v.year, v.month - 1, v.day));
  if (d.getUTCFullYear() !== v.year || d.getUTCMonth() !== v.month - 1 || d.getUTCDate() !== v.day) return null;
  return d.toISOString().slice(0, 10);
}

/**
 * (P5-REFINE-2, D-405) A date input in the person's locale: a text field that shows the browser's own order ("19/11/2026"
 * or "11/19/2026" — the placeholder says which) with a calendar button that opens the native picker. The value sent
 * stays ISO `YYYY-MM-DD`; an entry that is not a date leaves the value empty and marks the field invalid.
 */
export function DateField({
  value,
  onChange,
  "aria-label": ariaLabel,
  "aria-invalid": ariaInvalid,
  "aria-describedby": ariaDescribedBy,
  id,
  className,
}: {
  value: string;
  onChange: (iso: string) => void;
  "aria-label"?: string;
  "aria-invalid"?: boolean | "true" | "false";
  "aria-describedby"?: string;
  id?: string;
  className?: string;
}) {
  const [text, setText] = useState(() => isoToLocaleText(value));
  const [invalid, setInvalid] = useState(false);
  const picker = useRef<HTMLInputElement>(null);
  const placeholder = useMemo(() => {
    const { order, sep } = localeDatePattern();
    const word: Record<Part, string> = { day: "dd", month: "mm", year: "yyyy" };
    return order.map((p) => word[p]).join(sep);
  }, []);
  // Follow the value when it changes from outside (a Clear button, the picker).
  const [seen, setSeen] = useState(value);
  if (value !== seen) {
    setSeen(value);
    if (value !== (localeTextToIso(text) ?? "")) setText(isoToLocaleText(value));
  }
  function commit(next: string) {
    setText(next);
    if (next.trim() === "") {
      setInvalid(false);
      onChange("");
      return;
    }
    const iso = localeTextToIso(next);
    setInvalid(iso === null && /\d{1,4}\D+\d{1,2}\D+\d{1,4}/.test(next));
    if (iso !== null) onChange(iso);
  }
  return (
    <span className={cn("relative inline-flex items-center", className)}>
      <input
        id={id}
        type="text"
        inputMode="numeric"
        autoComplete="off"
        aria-label={ariaLabel}
        aria-invalid={invalid || ariaInvalid === true || ariaInvalid === "true" || undefined}
        aria-describedby={ariaDescribedBy}
        placeholder={placeholder}
        className={`${inputClass} w-40 pr-10`}
        value={text}
        onChange={(e) => commit(e.target.value)}
        onBlur={() => {
          if (text.trim() !== "" && localeTextToIso(text) === null) setInvalid(true);
        }}
      />
      <button
        type="button"
        aria-label={ariaLabel ? `Pick ${ariaLabel.toLowerCase()}` : "Pick a date"}
        className="absolute right-1 inline-flex size-8 items-center justify-center rounded-control text-ink-2 hover:bg-subtle hover:text-ink"
        onClick={() => {
          const el = picker.current;
          if (!el) return;
          try {
            el.showPicker();
          } catch {
            el.focus();
          }
        }}
      >
        <CalendarIcon className="size-4" />
      </button>
      <input
        ref={picker}
        type="date"
        tabIndex={-1}
        aria-hidden="true"
        className="pointer-events-none absolute right-1 top-full size-0 opacity-0"
        value={value}
        onChange={(e) => {
          setInvalid(false);
          setText(isoToLocaleText(e.target.value));
          onChange(e.target.value);
        }}
      />
    </span>
  );
}
