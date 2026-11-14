"use client";

import { useCallback, useState } from "react";

/** The rail's choice lives in this browser only (like the theme, `aurora.theme`) and is never sent to the API (D-201). */
export const RAIL_STORAGE_KEY = "aurora.agent-status-rail";

/** With nothing stored the rail starts expanded at a viewport of at least this many px. */
export const RAIL_DEFAULT_MIN_VIEWPORT = 1440;

export type RailPreference = "expanded" | "collapsed";

/** The stored choice, or `null` when none is stored or storage is unavailable (private window, blocked site data). */
export function readRailPreference(): RailPreference | null {
  try {
    const v = window.localStorage.getItem(RAIL_STORAGE_KEY);
    return v === "expanded" || v === "collapsed" ? v : null;
  } catch {
    return null;
  }
}

export function writeRailPreference(v: RailPreference): void {
  try {
    window.localStorage.setItem(RAIL_STORAGE_KEY, v);
  } catch {
    /* storage unavailable: the choice holds for this page view only */
  }
}

/** Expanded or not: the stored choice, else expanded from 1440 px of viewport. */
export function initialExpanded(stored: RailPreference | null, viewportWidth: number): boolean {
  if (stored) return stored === "expanded";
  return viewportWidth >= RAIL_DEFAULT_MIN_VIEWPORT;
}

export function useRailExpanded(): [boolean, (expanded: boolean) => void] {
  const [expanded, setExpanded] = useState(() =>
    typeof window === "undefined" ? false : initialExpanded(readRailPreference(), window.innerWidth),
  );
  const set = useCallback((next: boolean) => {
    setExpanded(next);
    writeRailPreference(next ? "expanded" : "collapsed");
  }, []);
  return [expanded, set];
}
