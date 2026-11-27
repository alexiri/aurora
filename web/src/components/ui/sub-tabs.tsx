"use client";

import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";

import { cn } from "@/lib/utils";

import { Tabs, type TabItem } from "./tabs";

/** How far from the edge the selected pill is brought, so the fade does not lie on it. */
const REVEAL_MARGIN = 32;

const strip = (root: HTMLElement | null) => root?.querySelector<HTMLElement>('[role="tablist"]') ?? null;

/**
 * The quiet second row under a tab (D-507, docs/14 §Components): the sub-pages of the open tab as pills (the agent
 * page's Computer pages and Settings sections). It is always one line: what does not fit scrolls sideways within the
 * row, with a fade at each edge that has more, and the selected pill is brought into view. The same tablist behaviour
 * as `Tabs` (arrows, Home, End; panels by `tabId`/`tabPanelId`).
 */
export function SubTabs({
  items,
  value,
  onValueChange,
  label,
  idBase,
  className,
}: {
  items: TabItem[];
  value: string;
  onValueChange: (v: string) => void;
  label: string;
  idBase?: string;
  className?: string;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const [more, setMore] = useState({ start: false, end: false });
  const measure = useCallback(() => {
    const el = strip(ref.current);
    if (!el) return;
    const start = el.scrollLeft > 1;
    const end = el.scrollLeft + el.clientWidth < el.scrollWidth - 1;
    setMore((prev) => (prev.start === start && prev.end === end ? prev : { start, end }));
  }, []);

  // Only the row scrolls (never `scrollIntoView`, which may move the page too).
  useLayoutEffect(() => {
    const el = strip(ref.current);
    const tab = el?.querySelector<HTMLElement>('[aria-selected="true"]');
    if (el && tab) {
      const row = el.getBoundingClientRect();
      const box = tab.getBoundingClientRect();
      if (box.left < row.left) el.scrollLeft -= row.left - box.left + REVEAL_MARGIN;
      else if (box.right > row.right) el.scrollLeft += box.right - row.right + REVEAL_MARGIN;
    }
    measure();
  }, [value, items.length, measure]);

  useEffect(() => {
    const el = strip(ref.current);
    if (!el) return;
    el.addEventListener("scroll", measure, { passive: true });
    // The web font may arrive after the first measurement and widen the pills.
    void document.fonts?.ready.then(measure);
    const ro = typeof ResizeObserver === "undefined" ? null : new ResizeObserver(measure);
    ro?.observe(el);
    return () => {
      el.removeEventListener("scroll", measure);
      ro?.disconnect();
    };
  }, [measure]);

  const fade = "pointer-events-none absolute inset-y-0 w-10 from-canvas to-transparent";
  return (
    <div ref={ref} className={cn("relative min-w-0", className)}>
      <Tabs variant="pills" current items={items} value={value} onValueChange={onValueChange} label={label} idBase={idBase} />
      {more.start ? <span aria-hidden="true" data-testid="sub-tabs-fade-start" className={cn(fade, "left-0 bg-linear-to-r")} /> : null}
      {more.end ? <span aria-hidden="true" data-testid="sub-tabs-fade-end" className={cn(fade, "right-0 bg-linear-to-l")} /> : null}
    </div>
  );
}
