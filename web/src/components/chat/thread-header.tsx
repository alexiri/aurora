"use client";

import type { ReactNode } from "react";

import { Badge } from "@/components/ui/badge";
import { ArrowLeftIcon, LockIcon } from "@/components/ui/icons";

/**
 * The thread header: a 44 px back button on phones, the title with one muted meta line (visibility, kind,
 * archived date, retention window), the connection pill and the actions slot.
 */
export function ThreadHeader({
  title,
  visibility,
  segments,
  live,
  onBack,
  actions,
}: {
  title: string;
  /** The visibility note, shown with a lock icon (direct conversations). */
  visibility?: string;
  /** The other meta segments (kind, archived date, retention). */
  segments: string[];
  /** `true` Live, `false` Reconnecting…, `null` no pill (loading or failed). */
  live: boolean | null;
  onBack?: () => void;
  actions?: ReactNode;
}) {
  return (
    <header className="flex shrink-0 items-center gap-2 border-b border-line px-3 py-2.5 md:px-5">
      {onBack ? (
        <button
          type="button"
          onClick={onBack}
          aria-label="Back to conversations"
          className="inline-flex size-11 shrink-0 items-center justify-center rounded-control text-ink-2 hover:bg-subtle hover:text-ink md:hidden"
        >
          <ArrowLeftIcon aria-hidden="true" width={20} height={20} />
        </button>
      ) : null}
      <div className="min-w-0 flex-1">
        <h2 className="truncate font-display text-base font-semibold text-ink">{title}</h2>
        <p className="flex min-w-0 flex-wrap items-center gap-x-1.5 text-xs text-muted">
          {visibility ? (
            <span className="inline-flex min-w-0 items-center gap-1">
              <LockIcon aria-hidden="true" width={12} height={12} className="shrink-0" />
              <span>{visibility}</span>
            </span>
          ) : null}
          {segments.map((seg, i) => (
            <span key={seg}>
              {visibility || i > 0 ? <span aria-hidden="true">· </span> : null}
              {seg}
            </span>
          ))}
        </p>
      </div>
      {live === null ? null : live ? <Badge tone="accent">Live</Badge> : <Badge tone="pending">Reconnecting…</Badge>}
      {actions}
    </header>
  );
}
