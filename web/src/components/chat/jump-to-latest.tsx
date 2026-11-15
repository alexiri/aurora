"use client";

import { ArrowDownIcon } from "@/components/ui/icons";

/** The pill over the transcript while the reader is scrolled up: "Jump to latest", with "· N new" when messages arrived. */
export function JumpToLatest({ count, onClick }: { count: number; onClick: () => void }) {
  return (
    <button
      type="button"
      onClick={onClick}
      className="absolute bottom-3 left-1/2 z-10 inline-flex min-h-11 -translate-x-1/2 items-center gap-1.5 rounded-full border border-line-strong bg-surface px-3.5 text-sm font-medium text-ink shadow-card hover:bg-subtle dark:bg-canvas md:min-h-9">
      <ArrowDownIcon aria-hidden="true" width={16} height={16} />
      <span>Jump to latest{count > 0 ? ` · ${count} new` : ""}</span>
    </button>
  );
}
