"use client";

import { useEffect, useRef, type ReactNode } from "react";

import { cn } from "@/lib/utils";

export interface Suggestion {
  /** Stable id of the option (also its DOM id suffix). */
  id: string;
  node: ReactNode;
}

/** The DOM id of option `i` of list `listId`, for the owner's `aria-activedescendant`. */
export const optionId = (listId: string, i: number) => `${listId}-opt-${i}`;

/**
 * A listbox of suggestions under (or over) a text field (docs/14 §Visual design). The field owns the keyboard
 * (`role="combobox"`, `aria-controls={id}`, `aria-activedescendant={optionId(id, active)}`); options are chosen with
 * the pointer without taking focus from the field. Used by the emoji `:` suggestions and the search box.
 */
export function SuggestionList({
  id,
  label,
  items,
  active,
  onChoose,
  className,
}: {
  id: string;
  label: string;
  items: Suggestion[];
  active: number;
  onChoose: (i: number) => void;
  className?: string;
}) {
  const ref = useRef<HTMLUListElement>(null);
  useEffect(() => {
    ref.current?.querySelector<HTMLElement>(`[id="${optionId(id, active)}"]`)?.scrollIntoView?.({ block: "nearest" });
  }, [active, id]);
  return (
    <ul
      ref={ref}
      id={id}
      role="listbox"
      aria-label={label}
      className={cn("max-h-60 overflow-y-auto rounded-card border border-line bg-surface p-1 text-sm text-ink shadow-pop", className)}>
      {items.map((it, i) => (
        <li
          key={it.id}
          id={optionId(id, i)}
          role="option"
          aria-selected={i === active}
          onMouseDown={(e) => {
            e.preventDefault();
            onChoose(i);
          }}
          className={cn("flex min-h-11 cursor-pointer items-center gap-2 rounded-control px-2 md:min-h-9", i === active ? "bg-subtle" : "hover:bg-subtle")}>
          {it.node}
        </li>
      ))}
    </ul>
  );
}
