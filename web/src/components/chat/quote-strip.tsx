"use client";

import { cn } from "@/lib/utils";

/** The `reply_to` of a `turn.queued` payload (C-725). */
export interface QuoteData {
  seq: number;
  author_label?: string;
  excerpt?: string;
  redacted?: boolean;
}

/** Reads `payload.reply_to` defensively: anything else is no quote. */
export function quoteOf(payload: Record<string, unknown>): QuoteData | null {
  const r = payload.reply_to;
  if (!r || typeof r !== "object") return null;
  const o = r as Record<string, unknown>;
  if (typeof o.seq !== "number") return null;
  return {
    seq: o.seq,
    author_label: typeof o.author_label === "string" ? o.author_label : undefined,
    excerpt: typeof o.excerpt === "string" ? o.excerpt : undefined,
    redacted: o.redacted === true,
  };
}

/**
 * The quote above a bubble that replies to a message: the author and a clamped excerpt (plain text, so a link in
 * the quote is inert), and a click or Enter jumps to the original. A redacted original shows a neutral line.
 */
export function QuoteStrip({ quote, onJump, side }: { quote: QuoteData; onJump: (seq: number) => void; side: "person" | "agent" }) {
  if (quote.redacted) {
    return (
      <p className="rounded-control border-l-2 border-line-strong px-2 py-1 text-xs italic text-muted" data-testid="quote-strip">
        Original message redacted
      </p>
    );
  }
  return (
    <button
      type="button"
      data-testid="quote-strip"
      onClick={(e) => {
        e.stopPropagation();
        onJump(quote.seq);
      }}
      aria-label={`Jump to the message from ${quote.author_label ?? "the original author"}`}
      className={cn(
        "block w-full min-w-0 rounded-control border-l-2 px-2 py-1 text-left text-xs",
        side === "person" ? "border-bubble-ink/60 bg-black/10 text-bubble-ink" : "border-accent bg-surface text-ink-2",
      )}>
      <span className="block font-semibold">{quote.author_label ?? "Message"}</span>
      <span className="line-clamp-3 block whitespace-pre-line break-words">{quote.excerpt ?? ""}</span>
    </button>
  );
}
