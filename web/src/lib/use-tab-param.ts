"use client";

import { useSearchParams } from "next/navigation";
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
 * The initial read prefers `useSearchParams`: during a client-side navigation (a `<Link>` click) the component can
 * mount while `window.location` still shows the previous page, so a plain read races the router and the tab from the
 * link's `?tab=` was lost (the guide's "Say hello" landed on Instructions). `useSearchParams` follows the navigation.
 *
 * Changes are written through the History API, which the App Router patches to keep `useSearchParams` in step (the
 * chat's `?c=` does the same). `router.replace` would start a navigation transition that races with those writes:
 * the chat's Back button lost its `?c=` removal to it.
 */
export function useTabParam<T extends string>(
  valid: readonly T[],
  fallback: T,
  opts: { initial?: (params: URLSearchParams) => T | null; drop?: (tab: T) => string[] } = {},
): [T, (tab: T) => void] {
  const searchParams = useSearchParams();
  const [tab, setTabState] = useState<T>(() => {
    const params = new URLSearchParams(currentSearch());
    // The router's value first: during a client navigation `window.location` can still show the previous page.
    const raw = searchParams.get("tab") ?? params.get("tab");
    if (raw) return resolveTab(raw, valid, fallback);
    return opts.initial?.(params) || fallback;
  });
  // A client-side navigation landed here with a `?tab=` the mount-time read missed (it raced the router):
  // adopt the router's value. Only when window.location carries no valid `tab` — after a local setTab it
  // already has the new one (written before the router catches up), and adopting then would undo the click.
  // (P5-REFINE-17, org-2) The URL and the visible tab always agree: whenever the router's `?tab=` changes (a `<Link>`
  // that only changes the search parameter, such as Set up on Google Drive), the tab follows it. A local `setTab`
  // writes the same value through the History API, so adopting it then changes nothing.
  const routerRaw = searchParams.get("tab");
  const [seenRaw, setSeenRaw] = useState(routerRaw);
  if (routerRaw !== seenRaw) {
    setSeenRaw(routerRaw);
    if (routerRaw && (valid as readonly string[]).includes(routerRaw) && routerRaw !== tab) setTabState(routerRaw as T);
  }
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
