"use client";

import { useEffect, useRef, useState, type KeyboardEventHandler, type ReactNode } from "react";

import { cn } from "@/lib/utils";

/** Below this window width the expanded status rail overlays the thread instead of pushing it (D-201, owner 2026-11-14). */
export const RAIL_OVERLAY_BELOW = 1100;

/**
 * The messaging panel (D-102): one card that fills the rest of the viewport below the agent header (at least
 * 560 px; CSS only). From 768 px the list, the thread and (phase 5, D-201) the status rail sit side by side; below it
 * the list and the thread are two views and `threadOpen` picks one, with no rail. The page does not scroll: the list,
 * the transcript and the rail scroll inside the panel.
 *
 * `rail` is given whether it should overlay (window narrower than 1100 px); while an expanded rail overlays, the list
 * and the thread are inert so focus stays in the rail.
 */
export function ChatPanel({
  list,
  thread,
  threadOpen,
  className,
  rail,
  railExpanded = false,
  listWide = false,
  onKeyDown,
}: {
  /** (P5-CHAT-4) Message search results show: the list pane is 320 px wide instead of 232 px (from 768 px). */
  listWide?: boolean;
  /** Keys pressed with focus anywhere in the panel (the search shortcuts). */
  onKeyDown?: KeyboardEventHandler<HTMLDivElement>;
  list: ReactNode;
  thread: ReactNode;
  threadOpen: boolean;
  className?: string;
  rail?: (overlay: boolean) => ReactNode;
  railExpanded?: boolean;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const [windowWidth, setWindowWidth] = useState<number | null>(null);
  useEffect(() => {
    const el = ref.current;
    if (!el || !rail) return;
    // The window's width decides (not the panel's): at 1280 px the expanded rail pushes the thread aside, like the board.
    const measure = () => setWindowWidth(window.innerWidth);
    measure();
    window.addEventListener("resize", measure);
    return () => window.removeEventListener("resize", measure);
  }, [rail]);
  // Unknown width (no layout yet, jsdom reports 1024 but renders nothing) pushes rather than overlays.
  const overlay = windowWidth !== null && windowWidth > 0 && windowWidth < RAIL_OVERLAY_BELOW;
  const covered = overlay && railExpanded;
  const inert = covered ? ({ inert: true } as object) : {};
  return (
    <div
      ref={ref}
      onKeyDown={onKeyDown}
      className={cn(
        "relative flex h-[calc(100dvh-21rem)] min-h-[560px] overflow-hidden rounded-card border border-line bg-surface shadow-card md:h-[calc(100dvh-7.5rem)]",
        className,
      )}>
      <div {...inert} className={cn("min-h-0 w-full shrink-0 flex-col border-line md:flex md:border-r", listWide ? "md:w-80" : "md:w-58", threadOpen ? "hidden" : "flex")}>
        {list}
      </div>
      <div {...inert} className={cn("min-h-0 min-w-0 flex-1 flex-col md:flex", threadOpen ? "flex" : "hidden")}>
        {thread}
      </div>
      {rail ? <div className="hidden md:contents">{rail(overlay)}</div> : null}
    </div>
  );
}
