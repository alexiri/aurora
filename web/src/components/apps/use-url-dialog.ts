"use client";

import { useSearchParams } from "next/navigation";
import { useCallback, useState } from "react";

/**
 * A dialog addressed by a query parameter (C-842: `?connect=<app>`, `?reconnect=<connection>`). It opens when the URL
 * carries the parameter (a deep link) or when `set` is called with a value (a button), and closing it removes the
 * parameter from the URL, so a reload does not reopen it. The URL is rewritten through the History API, which the App
 * Router keeps `useSearchParams` in step with (see `useTabParam`).
 */
export function useUrlDialog(name: string): [string | null, (value: string | null) => void] {
  const fromUrl = useSearchParams().get(name);
  const [value, setValue] = useState<string | null>(fromUrl);
  const [seen, setSeen] = useState(fromUrl);
  if (fromUrl !== seen) {
    // A navigation (or an agent's link) changed the parameter: follow it.
    setSeen(fromUrl);
    setValue(fromUrl);
  }
  const set = useCallback(
    (next: string | null) => {
      setValue(next);
      if (next !== null || typeof window === "undefined") return;
      const url = new URL(window.location.href);
      if (!url.searchParams.has(name)) return;
      url.searchParams.delete(name);
      window.history.replaceState(window.history.state, "", `${url.pathname}${url.search}${url.hash}`);
    },
    [name],
  );
  return [value, set];
}
