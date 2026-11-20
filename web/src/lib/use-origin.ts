"use client";

import { useSyncExternalStore } from "react";

const noop = () => () => undefined;

/** The page's origin (`https://aurora.example.org`), empty on the server so the first render matches. */
export function useOrigin(): string {
  return useSyncExternalStore(
    noop,
    () => window.location.origin,
    () => "",
  );
}
