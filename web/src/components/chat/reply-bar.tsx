"use client";

import { ReplyIcon } from "@/components/ui/icons";

import type { ReplyTarget } from "./chat-actions";

/** The strip above the composer's text field while a reply is set: author, a two-line excerpt and ✕ (Esc cancels). */
export function ReplyBar({ target, onCancel, error }: { target: ReplyTarget; onCancel: () => void; error?: string | null }) {
  return (
    <div className="mb-2 flex items-start gap-2 rounded-control border-l-2 border-accent bg-subtle px-2.5 py-1.5 text-sm" role="group" aria-label="Replying to">
      <ReplyIcon aria-hidden="true" width={16} height={16} className="mt-0.5 shrink-0 text-ink-2" />
      <div className="min-w-0 flex-1">
        <p className="text-xs font-semibold text-ink">Replying to {target.author}</p>
        <p className="line-clamp-2 whitespace-pre-line break-words text-xs text-ink-2">{target.excerpt}</p>
        {error ? (
          <p role="alert" className="mt-0.5 text-xs text-danger-ink">
            {error}
          </p>
        ) : null}
      </div>
      <button
        type="button"
        aria-label="Cancel reply"
        onClick={onCancel}
        className="inline-flex size-11 shrink-0 items-center justify-center rounded-control text-ink-2 hover:bg-surface hover:text-ink md:size-7">
        <span aria-hidden="true">✕</span>
      </button>
    </div>
  );
}
