"use client";

import { useCallback, useEffect, useState } from "react";

export type CopyState = "idle" | "copied" | "failed";

/** A copy button's words: "Copied" on success, "Copy failed" when the browser refused, else `idle`. */
export const copyLabel = (state: CopyState, idle = "Copy") =>
  state === "copied" ? "Copied" : state === "failed" ? "Copy failed" : idle;

/**
 * Copy text to the clipboard. `state` is "copied" only once the browser accepted the write, "failed" when it refused or
 * has no clipboard (an insecure origin, a blocked permission), and goes back to "idle" after `resetMs`.
 */
export function useCopy(resetMs = 2000): { state: CopyState; copy: (text: string) => void } {
  const [state, setState] = useState<CopyState>("idle");
  useEffect(() => {
    if (state === "idle") return;
    const t = setTimeout(() => setState("idle"), resetMs);
    return () => clearTimeout(t);
  }, [state, resetMs]);
  const copy = useCallback((text: string) => {
    let write: Promise<void> | undefined;
    try {
      write = navigator.clipboard?.writeText(text);
    } catch {
      write = undefined;
    }
    if (!write) return setState("failed");
    write.then(
      () => setState("copied"),
      () => setState("failed"),
    );
  }, []);
  return { state, copy };
}
