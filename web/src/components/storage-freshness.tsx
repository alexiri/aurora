"use client";

import { useEffect, useState } from "react";

import { Button } from "@/components/ui/button";
import { RefreshLimited } from "@/lib/api/org";

function updatedAgo(iso: string, now: number): string {
  const s = Math.max(0, Math.floor((now - new Date(iso).getTime()) / 1000));
  if (s < 60) return "just now";
  const m = Math.floor(s / 60);
  if (m < 60) return `${m} min ago`;
  const h = Math.floor(m / 60);
  return h < 24 ? `${h} h ago` : `${Math.floor(h / 24)} d ago`;
}

interface Refresher {
  mutate: () => void;
  isPending: boolean;
  error: unknown;
}

/**
 * "Updated <relative time>" and a Refresh button for a cached figure set (FIX-126). A refresh
 * refused by its rate class disables the button and says when it works again, instead of
 * showing a page error.
 */
export function StorageFreshness({ computedAt, refresh }: { computedAt: string; refresh: Refresher }) {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const t = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(t);
  }, []);
  const err = refresh.error;
  const wait = err instanceof RefreshLimited ? Math.max(0, Math.ceil((err.at + err.seconds * 1000 - now) / 1000)) : 0;
  const limited = wait > 0;
  const otherError = err && !(err instanceof RefreshLimited);
  return (
    <div className="flex flex-wrap items-center gap-3 text-sm text-ink-2">
      <span>Updated {updatedAgo(computedAt, now)}</span>
      <Button type="button" variant="outline" size="sm" disabled={refresh.isPending || limited} onClick={() => refresh.mutate()}>
        {refresh.isPending ? "Refreshing…" : "Refresh"}
      </Button>
      {limited ? (
        <span role="status">
          Refresh is limited; try again in {wait < 90 ? `${wait} s` : `${Math.ceil(wait / 60)} min`}.
        </span>
      ) : null}
      {otherError ? <span role="alert">Could not refresh. Try again in a moment.</span> : null}
    </div>
  );
}
