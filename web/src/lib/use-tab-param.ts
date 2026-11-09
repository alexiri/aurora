"use client";

import { useCallback, useState } from "react";

/** The tab a `?tab=` value selects: the value when it is one of `valid`, otherwise `fallback`. */
export function resolveTab<T extends string>(raw: string | null | undefined, valid: readonly T[], fallback: T): T {
  return raw && (valid as readonly string[]).includes(raw) ? (raw as T) : fallback;
}

const currentSearch = () => (typeof window === "undefined" ? "" : window.location.search);

/**
 * Tab state addressable by `?tab=` (C-506): read on load, and every change *replaces* the URL (no history entry). An
 * unknown value falls back. Other query parameters are kept, except those in `drop`.
 *
 * The URL is read and written through the History API, which the App Router patches to keep `useSearchParams` in step
 * (the chat's `?c=` does the same). `router.replace` would start a navigation transition that races with those
 * writes: the chat's Back button lost its `?c=` removal to it.
 */
export function useTabParam<T extends string>(
  valid: readonly T[],
  fallback: T,
  opts: { initial?: (params: URLSearchParams) => T | null; drop?: (tab: T) => string[] } = {},
): [T, (tab: T) => void] {
  const [tab, setTabState] = useState<T>(() => {
    const params = new URLSearchParams(currentSearch());
    const raw = params.get("tab");
    if (raw) return resolveTab(raw, valid, fallback);
    return opts.initial?.(params) || fallback;
  });
  const setTab = useCallback(
    (next: T) => {
      setTabState(next);
      const url = new URL(window.location.href);
      url.searchParams.set("tab", next);
      for (const k of opts.drop?.(next) ?? []) url.searchParams.delete(k);
      window.history.replaceState(window.history.state, "", `${url.pathname}${url.search}${url.hash}`);
    },
    // `opts` is a per-render literal; only its behaviour matters.
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [],
  );
  return [tab, setTab];
}
