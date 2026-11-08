import type { ReactNode } from "react";

import { cn } from "@/lib/utils";

/**
 * The messaging panel (D-102): one card that fills the rest of the viewport below the agent header (at least
 * 560 px; CSS only). From 768 px the list and the thread sit side by side; below it they are two views and
 * `threadOpen` picks one. The page does not scroll: the list and the transcript scroll inside the panel.
 */
export function ChatPanel({ list, thread, threadOpen, className }: { list: ReactNode; thread: ReactNode; threadOpen: boolean; className?: string }) {
  return (
    <div
      className={cn(
        "flex h-[calc(100dvh-21rem)] min-h-[560px] overflow-hidden rounded-card border border-line bg-surface shadow-card md:h-[calc(100dvh-18.5rem)]",
        className,
      )}
    >
      <div className={cn("min-h-0 w-full shrink-0 flex-col border-line md:flex md:w-72 md:border-r lg:w-80", threadOpen ? "hidden" : "flex")}>{list}</div>
      <div className={cn("min-h-0 min-w-0 flex-1 flex-col md:flex", threadOpen ? "flex" : "hidden")}>{thread}</div>
    </div>
  );
}
