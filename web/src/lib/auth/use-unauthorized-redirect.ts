"use client";

import { useQueryClient } from "@tanstack/react-query";
import { useEffect } from "react";

import { ApiError } from "@/lib/api/client";

/** Accept only same-origin absolute paths for `?next=`; anything else becomes `/`. */
export function safeNext(next: string | null | undefined): string {
  if (!next || !next.startsWith("/") || next.startsWith("//") || next.includes("\\")) return "/";
  return next;
}

/** Build `/auth/login?next=<path>`; `next` is always a same-origin absolute path. */
export function loginUrl(currentPath: string): string {
  const safe = safeNext(currentPath);
  return `/auth/login?next=${encodeURIComponent(safe)}`;
}

export function isUnauthorized(err: unknown): boolean {
  return err instanceof ApiError && err.status === 401;
}

/**
 * Redirect to the login page when any query or mutation fails with 401.
 * Mount once in each signed-in shell.
 */
export function useUnauthorizedRedirect(): void {
  const qc = useQueryClient();
  useEffect(() => {
    const go = (err: unknown) => {
      if (!isUnauthorized(err)) return;
      if (window.location.pathname.startsWith("/auth/")) return;
      window.location.assign(loginUrl(window.location.pathname + window.location.search));
    };
    const unsubQ = qc.getQueryCache().subscribe((e) => {
      if (e.type === "updated" && e.action.type === "error") go(e.action.error);
    });
    const unsubM = qc.getMutationCache().subscribe((e) => {
      if (e.type === "updated" && e.action.type === "error") go(e.action.error);
    });
    return () => {
      unsubQ();
      unsubM();
    };
  }, [qc]);
}
