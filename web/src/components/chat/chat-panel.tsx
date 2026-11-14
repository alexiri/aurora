"use client";

import { useEffect, useRef, useState, type ReactNode } from "react";

import { cn } from "@/lib/utils";

/** Below this panel width the expanded status rail overlays the thread instead of pushing it (D-201). */
export const RAIL_OVERLAY_BELOW = 1024;

/**
 * The messaging panel (D-102): one card that fills the rest of the viewport below the agent header (at least
 * 560 px; CSS only). From 768 px the list, the thread and (phase 5, D-201) the status rail sit side by side; below it
 * the list and the thread are two views and `threadOpen` picks one, with no rail. The page does not scroll: the list,
 * the transcript and the rail scroll inside the panel.
 *
 * `rail` is given whether it should overlay (panel narrower than 1024 px); while an expanded rail overlays, the list
 * and the thread are inert so focus stays in the rail.
 */
export function ChatPanel({
  list,
  thread,
  threadOpen,
  className,
  rail,
  railExpanded = false,
}: {
  list: ReactNode;
  thread: ReactNode;
  threadOpen: boolean;
  className?: string;
  rail?: (overlay: boolean) => ReactNode;
  railExpanded?: boolean;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const [width, setWidth] = useState<number | null>(null);
  useEffect(() => {
    const el = ref.current;
    if (!el || !rail) return;
    setWidth(el.clientWidth);
    if (typeof ResizeObserver === "undefined") return;
    const ro = new ResizeObserver(() => setWidth(el.clientWidth));
    ro.observe(el);
    return () => ro.disconnect();
  }, [rail]);
  // Unknown width (no layout yet, jsdom) pushes rather than overlays.
  const overlay = width !== null && width > 0 && width < RAIL_OVERLAY_BELOW;
  const covered = overlay && railExpanded;
  const inert = covered ? ({ inert: true } as object) : {};
  return (
    <div
      ref={ref}
      className={cn(
        "relative flex h-[calc(100dvh-21rem)] min-h-[560px] overflow-hidden rounded-card border border-line bg-surface shadow-card md:h-[calc(100dvh-14rem)]",
        className,
      )}
    >
      <div {...inert} className={cn("min-h-0 w-full shrink-0 flex-col border-line md:flex md:w-58 md:border-r", threadOpen ? "hidden" : "flex")}>
        {list}
      </div>
      <div {...inert} className={cn("min-h-0 min-w-0 flex-1 flex-col md:flex", threadOpen ? "flex" : "hidden")}>
        {thread}
      </div>
      {rail ? <div className="hidden md:contents">{rail(overlay)}</div> : null}
    </div>
  );
}
