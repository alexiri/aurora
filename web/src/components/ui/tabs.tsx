"use client";

import { useId, useRef, type KeyboardEvent, type ReactNode } from "react";

import { cn } from "@/lib/utils";

export interface TabItem {
  value: string;
  label: ReactNode;
}

/**
 * Accessible tablist (roving tabindex, arrow/Home/End keys). `segmented` is for
 * page-level views, `underline` for the agent page strip. Panels: give each
 * `id={tabPanelId(idBase, value)}`, `role="tabpanel"`, `aria-labelledby={tabId(idBase, value)}`.
 */
export function tabId(base: string, value: string) {
  return `${base}-tab-${value}`;
}
export function tabPanelId(base: string, value: string) {
  return `${base}-panel-${value}`;
}

export function Tabs({
  items,
  value,
  onValueChange,
  variant = "segmented",
  label,
  idBase,
  className,
}: {
  items: TabItem[];
  value: string;
  onValueChange: (v: string) => void;
  variant?: "segmented" | "underline";
  label: string;
  idBase?: string;
  className?: string;
}) {
  const auto = useId().replace(/:/g, "");
  const base = idBase ?? auto;
  const refs = useRef<Record<string, HTMLButtonElement | null>>({});

  function onKeyDown(e: KeyboardEvent) {
    const i = items.findIndex((t) => t.value === value);
    let next = -1;
    if (e.key === "ArrowRight") next = (i + 1) % items.length;
    else if (e.key === "ArrowLeft") next = (i - 1 + items.length) % items.length;
    else if (e.key === "Home") next = 0;
    else if (e.key === "End") next = items.length - 1;
    if (next < 0) return;
    e.preventDefault();
    const v = items[next].value;
    onValueChange(v);
    refs.current[v]?.focus();
  }

  return (
    <div
      role="tablist"
      aria-label={label}
      onKeyDown={onKeyDown}
      className={cn(
        variant === "segmented"
          ? "inline-flex max-w-full gap-0.5 overflow-x-auto rounded-control bg-subtle p-0.5"
          : "flex w-full min-w-0 max-w-full gap-1 overflow-x-auto overscroll-x-contain border-b border-line",
        className,
      )}
    >
      {items.map((t) => {
        const selected = t.value === value;
        return (
          <button
            key={t.value}
            ref={(el) => {
              refs.current[t.value] = el;
            }}
            type="button"
            role="tab"
            id={tabId(base, t.value)}
            aria-selected={selected}
            aria-controls={tabPanelId(base, t.value)}
            tabIndex={selected ? 0 : -1}
            onClick={() => onValueChange(t.value)}
            className={cn(
              "inline-flex min-h-9 shrink-0 items-center gap-2 whitespace-nowrap px-3 text-sm font-medium",
              variant === "segmented"
                ? cn("rounded-md", selected ? "bg-surface font-semibold text-ink shadow-card" : "text-ink-2 hover:text-ink")
                : cn(
                    "-mb-px border-b-2",
                    selected ? "border-accent font-semibold text-ink" : "border-transparent text-ink-2 hover:text-ink",
                  ),
            )}
          >
            {t.label}
          </button>
        );
      })}
    </div>
  );
}
