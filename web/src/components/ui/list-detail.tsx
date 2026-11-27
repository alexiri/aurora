"use client";

import { createContext, useContext, useEffect, type ReactNode } from "react";
import { createPortal } from "react-dom";

import { useMediaQuery } from "@/lib/use-media-query";
import { cn } from "@/lib/utils";

import { ChevronLeftIcon } from "./icons";

/**
 * (P5-MEMORY-2, M-9) A list with the selected item beside it, as two screens where both do not fit. Below
 * `NARROW_SCREEN` (a phone, and a small window where the rail and a settings sub-nav leave no room for two columns) the
 * list is one screen and the opened item is another, with a "‹ <list>" back link; the surrounding page chrome (the
 * settings header and sub-nav) steps aside on the item's screen. Only one of the two is rendered there.
 */
export const NARROW_SCREEN = "(max-width: 1099px)";

/** `false` on the server and in tests without `matchMedia`: the two-column layout is the first render. */
export function useNarrowScreen(): boolean {
  return useMediaQuery(NARROW_SCREEN);
}

export interface ScreenChrome {
  /** The place in the page header where a page puts its main action; null until it is mounted. */
  actionSlot: HTMLElement | null;
  /** Hides the page header and sub-navigation while a sub-screen is open. */
  setSubScreen: (open: boolean) => void;
}

export const ScreenChromeContext = createContext<ScreenChrome | null>(null);

/** The page's main action, shown in the surrounding page header when there is one, else at the top right here. */
export function HeaderActions({ children }: { children: ReactNode }) {
  const chrome = useContext(ScreenChromeContext);
  if (!chrome) return <div className="flex justify-end">{children}</div>;
  return chrome.actionSlot ? createPortal(children, chrome.actionSlot) : null;
}

/** The top of an item's own screen on a narrow screen: "‹ <list>" back, with the item's main actions beside it. */
export function SubScreenHeader({ backLabel, onBack, children }: { backLabel: string; onBack: () => void; children?: ReactNode }) {
  return (
    <div className="flex min-h-11 items-center justify-between gap-2 border-b border-line pb-2">
      <button type="button" onClick={onBack} className="inline-flex min-h-11 items-center gap-1 text-sm font-semibold text-accent-ink">
        <ChevronLeftIcon className="size-4" aria-hidden="true" />
        {backLabel}
      </button>
      <div className="flex shrink-0 items-center gap-2">{children}</div>
    </div>
  );
}

export function ListDetail({
  narrow,
  detailOpen,
  list,
  detail,
  className,
}: {
  narrow: boolean;
  /** Narrow screens only: whether the item's screen is open. */
  detailOpen: boolean;
  list: ReactNode;
  /** On a narrow screen it starts with a `SubScreenHeader`. */
  detail: ReactNode;
  className?: string;
}) {
  const chrome = useContext(ScreenChromeContext);
  const sub = narrow && detailOpen;
  const setSubScreen = chrome?.setSubScreen;
  useEffect(() => {
    if (!sub || !setSubScreen) return;
    setSubScreen(true);
    return () => setSubScreen(false);
  }, [sub, setSubScreen]);
  // Opening an item starts its screen at the top.
  useEffect(() => {
    if (sub) window.scrollTo?.({ top: 0 });
  }, [sub]);

  // One tree for both layouts, the panes told apart by their keys: crossing the breakpoint (a window dragged, a phone
  // turned) must not mount the item's pane again, or a form in it loses what was typed.
  return (
    <div className={cn(narrow ? "min-w-0" : "grid items-start gap-4 grid-cols-[minmax(0,18rem)_minmax(0,1fr)]", className)}>
      {narrow && detailOpen ? null : (
        <div key="list" className="min-w-0">
          {list}
        </div>
      )}
      {narrow && !detailOpen ? null : (
        <div key="detail" className="min-w-0">
          {detail}
        </div>
      )}
    </div>
  );
}

/** On a narrow screen an item is the page itself, not a card on it. The same element either way (see `ListDetail`). */
export const BARE_ON_NARROW = "border-0 bg-transparent p-0 shadow-none sm:p-0";
