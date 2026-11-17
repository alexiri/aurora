"use client";

import type { ReactNode } from "react";

import { Button } from "@/components/ui/button";

/**
 * (C-989) The one pending-changes bar of a page: "<n> changes not applied yet" with **Discard** and **Apply changes**.
 * It is not rendered when nothing is pending. `children` is a sentence about a part that failed (the organization
 * page names which part was applied) and sits above the bar's buttons.
 */
export function ApplyBar({
  count,
  busy = false,
  onApply,
  onDiscard,
  children,
}: {
  count: number;
  busy?: boolean;
  onApply: () => void;
  onDiscard: () => void;
  children?: ReactNode;
}) {
  if (count === 0) return null;
  return (
    <div
      role="region"
      aria-label="Changes not applied"
      className="sticky bottom-4 z-10 flex flex-wrap items-center gap-x-3 gap-y-2 rounded-card bg-primary p-3 text-on-primary shadow-card sm:px-4"
    >
      <p className="min-w-48 flex-1 font-semibold">
        {count === 1 ? "1 change not applied yet" : `${count} changes not applied yet`}
        {children ? <span className="mt-0.5 block text-sm font-normal">{children}</span> : null}
      </p>
      <Button type="button" size="sm" variant="ghost" className="border border-current text-on-primary hover:bg-transparent hover:text-on-primary hover:opacity-80" disabled={busy} onClick={onDiscard}>
        Discard
      </Button>
      <Button type="button" size="sm" variant="outline" className="border-transparent bg-on-primary text-primary" disabled={busy} onClick={onApply}>
        {busy ? "Applying…" : "Apply changes"}
      </Button>
    </div>
  );
}
