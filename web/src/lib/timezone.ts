"use client";

import { useQueryClient } from "@tanstack/react-query";
import { useEffect, useRef } from "react";

import { api } from "@/lib/api/client";
import { useMe } from "@/lib/api/queries";
import { unwrap } from "@/lib/api/unwrap";
import { setDisplayTimeZone } from "@/lib/format-date";

/** Per-browser-session marker: the auto-fill below runs at most once per session. */
const SESSION_KEY = "aurora.timezone-autofill";

/** True when `tz` is an IANA zone name this browser knows (`Area/City` or `UTC`). */
export function isIanaZone(tz: string | null | undefined): tz is string {
  if (!tz || tz === "Etc/Unknown") return false;
  if (tz !== "UTC" && !tz.includes("/")) return false;
  try {
    new Intl.DateTimeFormat("en-US", { timeZone: tz });
    return true;
  } catch {
    return false;
  }
}

/** The browser's own time zone, when it is a valid IANA name. */
export function browserTimeZone(): string | null {
  try {
    const tz = Intl.DateTimeFormat().resolvedOptions().timeZone;
    return isIanaZone(tz) ? tz : null;
  } catch {
    return null;
  }
}

function alreadyRan(): boolean {
  try {
    return window.sessionStorage.getItem(SESSION_KEY) === "1";
  } catch {
    return false;
  }
}

function markRan() {
  try {
    window.sessionStorage.setItem(SESSION_KEY, "1");
  } catch {
    // Storage unavailable: the ref below still keeps it to once per page load.
  }
}

/**
 * (P5-PERSON, D-402(d), C-1054) When the person has no time zone yet (`GET /v1/me` says
 * `timezone: null`), fill it once per browser session from the browser's zone. Never when it is
 * set (or the API does not report it), never with a value that is not a valid IANA name.
 */
export function useTimezoneAutofill(): void {
  const me = useMe().data;
  const qc = useQueryClient();
  const ran = useRef(false);
  // (P5-REFINE-2, D-405) Every time on screen is shown in the person's zone (else the browser's).
  setDisplayTimeZone(me?.timezone);
  useEffect(() => {
    if (ran.current || !me || me.timezone !== null || alreadyRan()) return;
    const tz = browserTimeZone();
    if (!tz) return;
    ran.current = true;
    markRan();
    void unwrap(api.PATCH("/v1/me", { body: { timezone: tz } }))
      .then(() => qc.invalidateQueries({ queryKey: ["me"] }))
      .catch(() => undefined);
  }, [me, qc]);
}
