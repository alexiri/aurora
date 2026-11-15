"use client";

import { useCallback, useEffect, useState } from "react";

import { WORKSPACE_ROOT, normalizePath } from "@/lib/api/files";

const read = (): string => {
  if (typeof window === "undefined") return WORKSPACE_ROOT;
  return normalizePath(new URLSearchParams(window.location.search).get("path") ?? "") ?? WORKSPACE_ROOT;
};

/** The URL that shows `path` on the Files page (`?tab=files&path=`, root = no `path`), for links that survive a new tab. */
export function filesHref(path: string): string {
  const params = new URLSearchParams(typeof window === "undefined" ? "" : window.location.search);
  params.set("tab", "files");
  if (path === WORKSPACE_ROOT) params.delete("path");
  else params.set("path", path);
  return `?${params.toString()}`;
}

/**
 * The folder the Files page shows, kept in the URL (`&path=`, docs/14 §Files item 1) so reload and Back keep the place.
 * Opening a folder pushes a history entry (Back goes up the trail); the History API write is the one `useTabParam` uses,
 * which the App Router keeps in step. A `path` that is not under `/workspace` falls back to the root.
 */
export function useFilesPath(): [string, (path: string) => void] {
  const [path, setPathState] = useState<string>(read);
  useEffect(() => {
    const onPop = () => setPathState(read());
    window.addEventListener("popstate", onPop);
    return () => window.removeEventListener("popstate", onPop);
  }, []);
  const setPath = useCallback((next: string) => {
    setPathState(next);
    const url = new URL(window.location.href);
    if (next === WORKSPACE_ROOT) url.searchParams.delete("path");
    else url.searchParams.set("path", next);
    window.history.pushState(window.history.state, "", `${url.pathname}${url.search}${url.hash}`);
  }, []);
  return [path, setPath];
}
