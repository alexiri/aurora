"use client";

import { useCallback, useEffect, useId, useLayoutEffect, useRef, useState, type KeyboardEvent, type ReactNode } from "react";

import { cn } from "@/lib/utils";

import { ChevronDownIcon } from "./icons";
import { Menu, MenuContent, MenuItem, MenuTrigger } from "./menu";

export interface TabItem {
  value: string;
  label: ReactNode;
}

/**
 * Accessible tablist (roving tabindex, arrow/Home/End keys). `segmented` is for
 * page-level views, `underline` for the agent page strip. Panels: give each
 * `id={tabPanelId(idBase, value)}`, `role="tabpanel"`, `aria-labelledby={tabId(idBase, value)}`.
 *
 * `overflow="menu"` (docs/14 §Tabs): tabs that do not fit collapse into a trailing **More** menu instead of a
 * sideways-scrolling strip. The selected tab is always shown (in place of the last visible tab if it lives in
 * the menu); arrow keys still move across every tab.
 */
export function tabId(base: string, value: string) {
  return `${base}-tab-${value}`;
}
export function tabPanelId(base: string, value: string) {
  return `${base}-panel-${value}`;
}

/** Gap between tabs in px (`gap-1`); the measurement and `visibleTabs` use the same number. */
const GAP = 4;

/**
 * Which tabs (original indices, in order) are shown when `available` px are free and the trailing More button is
 * `moreWidth` px wide. All of them when they fit; otherwise as many from the start as fit beside More, with the
 * selected tab kept visible (it replaces the tabs that no longer fit). Pure, so it is unit tested without layout.
 */
export function visibleTabs(widths: number[], available: number, moreWidth: number, selected: number, gap = GAP): number[] {
  const all = widths.map((_, i) => i);
  // No layout to measure (not rendered, display: none, jsdom): show everything rather than collapse.
  if (widths.length === 0 || available <= 0) return all;
  const total = widths.reduce((a, b) => a + b, 0) + gap * (widths.length - 1);
  if (total <= available) return all;
  const fill = (budget: number, skip: number): number[] => {
    const out: number[] = [];
    let used = 0;
    for (let i = 0; i < widths.length; i += 1) {
      if (i === skip) continue;
      const w = widths[i]! + (out.length > 0 ? gap : 0);
      if (used + w > budget) break;
      out.push(i);
      used += w;
    }
    return out;
  };
  const budget = available - moreWidth - gap;
  const prefix = fill(budget, -1);
  if (selected < 0 || prefix.includes(selected)) return prefix.length > 0 ? prefix : [Math.max(0, selected)];
  const rest = fill(budget - widths[selected]! - gap, selected);
  return [...rest, selected];
}

function tabClass(variant: "segmented" | "underline", selected: boolean, overlapBorder = true) {
  return cn(
    "inline-flex min-h-9 shrink-0 items-center gap-2 whitespace-nowrap px-3 text-sm font-medium",
    variant === "segmented"
      ? cn("rounded-md", selected ? "bg-surface font-semibold text-ink shadow-card" : "text-ink-2 hover:text-ink")
      : cn(
          "border-b-2",
          overlapBorder && "-mb-px",
          selected ? "border-accent font-semibold text-ink" : "border-transparent text-ink-2 hover:text-ink",
        ),
  );
}

export function Tabs({
  items,
  value,
  onValueChange,
  variant = "segmented",
  label,
  idBase,
  className,
  overflow = "scroll",
}: {
  items: TabItem[];
  value: string;
  onValueChange: (v: string) => void;
  variant?: "segmented" | "underline";
  label: string;
  idBase?: string;
  className?: string;
  /** `scroll` (default): the strip scrolls within itself. `menu`: what does not fit moves into a More menu. */
  overflow?: "scroll" | "menu";
}) {
  const auto = useId().replace(/:/g, "");
  const base = idBase ?? auto;
  const refs = useRef<Record<string, HTMLButtonElement | null>>({});
  const focusAfter = useRef<string | null>(null);
  const useMenu = overflow === "menu";

  // ---- overflow measurement (menu mode) ----
  const wrapRef = useRef<HTMLDivElement>(null);
  const measureRef = useRef<HTMLDivElement>(null);
  const [shown, setShown] = useState<number[] | null>(null);
  const selectedIndex = items.findIndex((t) => t.value === value);
  const recompute = useCallback(() => {
    const wrap = wrapRef.current;
    const measure = measureRef.current;
    if (!wrap || !measure) return;
    const cells = Array.from(measure.querySelectorAll<HTMLElement>("[data-measure-tab]"));
    const more = measure.querySelector<HTMLElement>("[data-measure-more]");
    const widths = cells.map((c) => c.offsetWidth);
    const next = visibleTabs(widths, wrap.clientWidth, more?.offsetWidth ?? 0, selectedIndex);
    setShown((prev) => (prev && prev.length === next.length && prev.every((v, i) => v === next[i]) ? prev : next));
  }, [selectedIndex]);
  useLayoutEffect(() => {
    if (useMenu) recompute();
  }, [useMenu, recompute, items]);
  useEffect(() => {
    if (!useMenu || !wrapRef.current) return;
    const el = wrapRef.current;
    if (typeof ResizeObserver !== "undefined") {
      const ro = new ResizeObserver(() => recompute());
      ro.observe(el);
      return () => ro.disconnect();
    }
    window.addEventListener("resize", recompute);
    return () => window.removeEventListener("resize", recompute);
  }, [useMenu, recompute]);

  // After a keyboard move onto a tab that was in the menu it is rendered in the strip: focus it then.
  useEffect(() => {
    const v = focusAfter.current;
    if (v && refs.current[v]) {
      refs.current[v]?.focus();
      focusAfter.current = null;
    }
  });

  const visibleIdx = useMenu && shown ? shown : items.map((_, i) => i);
  const inStrip = new Set(visibleIdx);
  const hidden = items.filter((_, i) => !inStrip.has(i));

  function onKeyDown(e: KeyboardEvent) {
    const i = items.findIndex((t) => t.value === value);
    let next = -1;
    if (e.key === "ArrowRight") next = (i + 1) % items.length;
    else if (e.key === "ArrowLeft") next = (i - 1 + items.length) % items.length;
    else if (e.key === "Home") next = 0;
    else if (e.key === "End") next = items.length - 1;
    if (next < 0) return;
    e.preventDefault();
    const v = items[next]!.value;
    focusAfter.current = v;
    onValueChange(v);
    refs.current[v]?.focus();
  }

  const stripClass = cn(
    variant === "segmented"
      ? "inline-flex max-w-full gap-0.5 rounded-control bg-subtle p-0.5"
      : cn("flex w-full min-w-0 max-w-full gap-1", !useMenu && "border-b border-line"),
    useMenu ? "overflow-hidden" : variant === "segmented" ? "overflow-x-auto" : "overflow-x-auto overscroll-x-contain",
  );

  const tablist = (
    <div role="tablist" aria-label={label} onKeyDown={onKeyDown} className={cn(stripClass, useMenu && "min-w-0 flex-1", className)}>
      {visibleIdx.map((i) => {
        const t = items[i]!;
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
            className={tabClass(variant, selected, !useMenu)}
          >
            {t.label}
          </button>
        );
      })}
    </div>
  );
  if (!useMenu) return tablist;

  const moreClass = cn(tabClass(variant, false, false), "gap-1");
  return (
    <div ref={wrapRef} className={cn("relative flex w-full min-w-0 max-w-full items-end", variant === "underline" && "border-b border-line")}>
      {/* The wrapper draws the underline, so it runs under the More button too. */}
      <div className="min-w-0 flex-1">{tablist}</div>
      {hidden.length > 0 ? (
        <Menu>
          <MenuTrigger asChild>
            <button type="button" className={moreClass} aria-label={`More ${label.toLowerCase()} tabs`}>
              More
              <ChevronDownIcon className="size-4" />
            </button>
          </MenuTrigger>
          <MenuContent align="end">
            {hidden.map((t) => (
              <MenuItem
                key={t.value}
                onSelect={() => {
                  focusAfter.current = t.value;
                  onValueChange(t.value);
                }}
              >
                {t.label}
              </MenuItem>
            ))}
          </MenuContent>
        </Menu>
      ) : null}
      {/* Off-screen copy of every tab and of More, only measured (never focusable or announced). */}
      <div
        ref={measureRef}
        aria-hidden="true"
        // `inert` keeps the copies out of the tab order and the accessibility tree.
        {...({ inert: true } as object)}
        className="pointer-events-none invisible absolute left-0 top-0 -z-10 flex h-0 gap-1 overflow-hidden"
      >
        {items.map((t) => (
          <span key={t.value} data-measure-tab className={tabClass(variant, t.value === value, false)}>
            {t.label}
          </span>
        ))}
        <span data-measure-more className={moreClass}>
          More
          <ChevronDownIcon className="size-4" />
        </span>
      </div>
    </div>
  );
}
