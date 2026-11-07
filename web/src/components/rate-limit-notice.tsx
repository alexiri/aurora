"use client";

import { useRateNotice } from "@/lib/api/rate-limit";

/** Non-blocking notice for API rate limiting (RFE-017, docs/14 §Networking). */
export function RateLimitNotice() {
  const n = useRateNotice();
  if (!n) return null;
  return (
    <div
      role="status"
      aria-live="polite"
      className="fixed bottom-4 right-4 z-50 rounded-md border bg-background px-4 py-2 text-sm shadow-md"
    >
      {n.kind === "retrying"
        ? `You're going fast — retrying in ${n.seconds}s`
        : "Too many open tabs or streams. Close some and reload."}
    </div>
  );
}
