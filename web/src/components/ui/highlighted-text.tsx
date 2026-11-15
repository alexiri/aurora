import { Fragment } from "react";

import { cn } from "@/lib/utils";

/**
 * Plain text with ranges marked (`mark`; docs/14 §Message search). `highlights` are `[start, end)` offsets in UTF-16
 * code units, which is what JavaScript strings index. The text is never parsed as markup; bad or overlapping ranges
 * are clamped or skipped, so a wrong server answer cannot throw.
 */
export function HighlightedText({ text, highlights, className }: { text: string; highlights: number[][]; className?: string }) {
  const ranges = highlights
    .map(([s, e]) => [Math.max(0, Math.min(s ?? 0, text.length)), Math.max(0, Math.min(e ?? 0, text.length))] as const)
    .filter(([s, e]) => e > s)
    .sort((a, b) => a[0] - b[0]);
  const parts: { text: string; mark: boolean }[] = [];
  let at = 0;
  for (const [s, e] of ranges) {
    if (s < at) continue;
    if (s > at) parts.push({ text: text.slice(at, s), mark: false });
    parts.push({ text: text.slice(s, e), mark: true });
    at = e;
  }
  if (at < text.length) parts.push({ text: text.slice(at), mark: false });
  return (
    <span className={className}>
      {parts.map((p, i) =>
        p.mark ? (
          <mark key={i} className={cn("rounded-sm bg-accent-soft px-0.5 font-semibold text-accent-ink")}>
            {p.text}
          </mark>
        ) : (
          <Fragment key={i}>{p.text}</Fragment>
        ),
      )}
    </span>
  );
}
