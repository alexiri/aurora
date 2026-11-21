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

/** `YYYY-MM-DD` → the app's own date text ("19 Nov 2026", as `formatDay` writes it), "" for nothing. */
export function isoToAppText(iso: string, locale?: string): string {
  const m = /^(\d{4})-(\d{2})-(\d{2})$/.exec(iso);
  if (!m) return "";
  return new Intl.DateTimeFormat(locale, { day: "numeric", month: "short", year: "numeric", timeZone: "UTC" }).format(
    new Date(Date.UTC(Number(m[1]), Number(m[2]) - 1, Number(m[3]))),
  );
}

/** The names a month goes by in the locale, short and long, lowercased and without a trailing dot. */
function monthNames(locale?: string): string[][] {
  const short = new Intl.DateTimeFormat(locale, { month: "short", timeZone: "UTC" });
  const long = new Intl.DateTimeFormat(locale, { month: "long", timeZone: "UTC" });
  return Array.from({ length: 12 }, (_, i) => {
    const d = new Date(Date.UTC(2026, i, 15));
    return [short.format(d), long.format(d)].map((s) => s.toLowerCase().replace(/\.$/, ""));
  });
}

/** The app's date text ("19 Nov 2026" or "Nov 19, 2026"), an ISO date, or the locale's numbers → `YYYY-MM-DD`; `null` when it is not a real date. */
export function appTextToIso(text: string, locale?: string): string | null {
  const t = text.trim();
  const iso = /^(\d{4})-(\d{2})-(\d{2})$/.exec(t);
  if (iso) {
    const d = new Date(Date.UTC(Number(iso[1]), Number(iso[2]) - 1, Number(iso[3])));
    return d.getUTCFullYear() === Number(iso[1]) && d.getUTCMonth() === Number(iso[2]) - 1 && d.getUTCDate() === Number(iso[3]) ? t : null;
  }
  const tokens = t.match(/\p{L}+\.?|\d+/gu) ?? [];
  const words = tokens.filter((x) => /^\p{L}/u.test(x));
  const nums = tokens.filter((x) => /^\d/.test(x));
  if (words.length === 1 && nums.length === 2) {
    const w = words[0]!.toLowerCase().replace(/\.$/, "");
    const month = monthNames(locale).findIndex((names) => names.includes(w));
    const year = nums.find((n) => n.length === 4) ?? nums.find((n) => Number(n) > 31);
    const day = nums.find((n) => n !== year);
    if (month < 0 || !year || !day) return null;
    const d = new Date(Date.UTC(Number(year), month, Number(day)));
    if (d.getUTCFullYear() !== Number(year) || d.getUTCMonth() !== month || d.getUTCDate() !== Number(day)) return null;
    return d.toISOString().slice(0, 10);
  }
  return localeTextToIso(text, locale);
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
  format = "locale",
}: {
  value: string;
  onChange: (iso: string) => void;
  "aria-label"?: string;
  "aria-invalid"?: boolean | "true" | "false";
  "aria-describedby"?: string;
  id?: string;
  className?: string;
  /** `"app"` writes and reads the date as the app shows every date ("19 Nov 2026"); the default is the browser's numeric order. */
  format?: "locale" | "app";
}) {
  const app = format === "app";
  const toText = (iso: string) => (app ? isoToAppText(iso) : isoToLocaleText(iso));
  const toIso = (t: string) => (app ? appTextToIso(t) : localeTextToIso(t));
  const [text, setText] = useState(() => toText(value));
  const [invalid, setInvalid] = useState(false);
  const picker = useRef<HTMLInputElement>(null);
  const placeholder = useMemo(() => {
    if (app) return isoToAppText("2026-11-19");
    const { order, sep } = localeDatePattern();
    const word: Record<Part, string> = { day: "dd", month: "mm", year: "yyyy" };
    return order.map((p) => word[p]).join(sep);
  }, [app]);
  // Follow the value when it changes from outside (a Clear button, the picker).
  const [seen, setSeen] = useState(value);
  if (value !== seen) {
    setSeen(value);
    if (value !== (toIso(text) ?? "")) setText(toText(value));
  }
  function commit(next: string) {
    setText(next);
    if (next.trim() === "") {
      setInvalid(false);
      onChange("");
      return;
    }
    const iso = toIso(next);
    // While typing, only a full numeric date that is not a date is marked; the app's text is judged when the field is left.
    setInvalid(iso === null && !app && /\d{1,4}\D+\d{1,2}\D+\d{1,4}/.test(next));
    if (iso !== null) onChange(iso);
  }
  return (
    <span className={cn("relative inline-flex items-center", className)}>
      <input
        id={id}
        type="text"
        inputMode={app ? "text" : "numeric"}
        autoComplete="off"
        aria-label={ariaLabel}
        aria-invalid={invalid || ariaInvalid === true || ariaInvalid === "true" || undefined}
        aria-describedby={ariaDescribedBy}
        placeholder={placeholder}
        className={`${inputClass} w-40 pr-10`}
        value={text}
        onChange={(e) => commit(e.target.value)}
        onBlur={() => {
          if (text.trim() !== "" && toIso(text) === null) setInvalid(true);
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
          setText(toText(e.target.value));
          onChange(e.target.value);
        }}
      />
    </span>
  );
}
