"use client";

import { useId, useMemo, useState, type ComponentProps } from "react";

import { inputClass } from "@/components/ui/field";
import { ChevronsUpDownIcon } from "@/components/ui/icons";
import { matchesFilter } from "@/components/ui/picker-filter";
import { optionId, SuggestionList } from "@/components/ui/suggestion-list";
import { cn } from "@/lib/utils";

export interface ComboboxOption {
  value: string;
  /** What the row and the closed field show; typing filters on it and on `value`. */
  label: string;
}

/** Rows drawn at once; a longer list is narrowed by typing. */
const MAX_ROWS = 200;

/**
 * (P5-REFINE-14 send-back) A searchable select: the dashboard's `Select` look, but you can type to narrow a long list
 * (the time zones). Closed, it shows the chosen label; focused, typing filters; Up/Down move, Enter chooses, Escape
 * closes and keeps the choice, Tab leaves. The list is a `SuggestionList` (tokens, both themes). Give it an `id` and a
 * `<Field>` label, or an `aria-label`. `value` is always one of `options` (there is no free text).
 */
export function Combobox({
  options,
  value,
  onChange,
  className,
  wrapperClassName,
  id,
  emptyText = "No match",
  ...props
}: Omit<ComponentProps<"input">, "onChange" | "value" | "type" | "role"> & {
  options: readonly ComboboxOption[];
  value: string;
  onChange: (value: string) => void;
  emptyText?: string;
  /** Classes of the box around the field and its list (width in a row). */
  wrapperClassName?: string;
}) {
  const auto = useId();
  const listId = `${id ?? auto}-list`;
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState<string | null>(null);
  const [active, setActive] = useState(0);
  const chosen = options.find((o) => o.value === value);
  const shown = useMemo(
    () => (query === null ? options : options.filter((o) => matchesFilter(query, o.label, o.value))).slice(0, MAX_ROWS),
    [options, query],
  );

  const close = () => {
    setOpen(false);
    setQuery(null);
  };
  const choose = (i: number) => {
    const o = shown[i];
    if (o) onChange(o.value);
    close();
  };

  return (
    <div className={cn("relative w-full", wrapperClassName)}>
      <input
        {...props}
        id={id}
        type="text"
        role="combobox"
        autoComplete="off"
        aria-autocomplete="list"
        aria-expanded={open}
        aria-controls={listId}
        aria-activedescendant={open && shown.length > 0 ? optionId(listId, Math.min(active, shown.length - 1)) : undefined}
        className={cn(inputClass, "pr-9", className)}
        value={query ?? chosen?.label ?? value}
        onFocus={(e) => {
          setOpen(true);
          setActive(Math.max(0, options.findIndex((o) => o.value === value)));
          e.currentTarget.select();
        }}
        onBlur={close}
        onChange={(e) => {
          setQuery(e.target.value);
          setOpen(true);
          setActive(0);
        }}
        onKeyDown={(e) => {
          if (e.key === "ArrowDown" || e.key === "ArrowUp") {
            e.preventDefault();
            if (!open) {
              setOpen(true);
              return;
            }
            const n = shown.length;
            if (n > 0) setActive((a) => (e.key === "ArrowDown" ? (a + 1) % n : (a - 1 + n) % n));
          } else if (e.key === "Enter" && open) {
            e.preventDefault();
            if (shown.length > 0) choose(Math.min(active, shown.length - 1));
          } else if (e.key === "Escape" && open) {
            e.preventDefault();
            e.stopPropagation();
            close();
          }
        }}
      />
      <ChevronsUpDownIcon aria-hidden className="pointer-events-none absolute right-3 top-1/2 size-4 -translate-y-1/2 text-muted" />
      {open ? (
        shown.length > 0 ? (
          <SuggestionList
            id={listId}
            label={props["aria-label"] ? `${props["aria-label"]} choices` : "Choices"}
            className="absolute z-40 mt-1 w-full"
            items={shown.map((o) => ({ id: o.value, node: <span className={cn("min-w-0 truncate", o.value === value && "font-medium")}>{o.label}</span> }))}
            active={Math.min(active, shown.length - 1)}
            onChoose={choose}
          />
        ) : (
          <p id={listId} role="status" className="absolute z-40 mt-1 w-full rounded-card border border-line bg-surface p-3 text-sm text-muted shadow-pop">
            {emptyText}
          </p>
        )
      ) : null}
    </div>
  );
}
