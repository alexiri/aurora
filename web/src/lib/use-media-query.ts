"use client";

import { useSyncExternalStore } from "react";

/**
 * Whether the viewport matches `query`. `false` on the server and wherever `matchMedia` is missing (jsdom), so the
 * first render is the narrow one and the wide one swaps in after hydration — a `matchMedia`-only decision, never
 * duplicated markup (what a `hidden … xl:inline` twin would give).
 */
export function useMediaQuery(query: string): boolean {
  return useSyncExternalStore(
    (onStoreChange) => {
      if (typeof window === "undefined" || typeof window.matchMedia !== "function") return () => undefined;
      const m = window.matchMedia(query);
      m.addEventListener("change", onStoreChange);
      return () => m.removeEventListener("change", onStoreChange);
    },
    () => typeof window !== "undefined" && typeof window.matchMedia === "function" && window.matchMedia(query).matches,
    () => false,
  );
}
