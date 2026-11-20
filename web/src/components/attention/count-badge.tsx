"use client";

import { AttentionChip } from "@/components/ui/attention-chip";

/** "1 needs your attention" / "2 need your attention": the words a screen reader gets behind a count. */
export const attentionWords = (n: number): string => (n === 1 ? "1 needs your attention" : `${n} need your attention`);

/**
 * (P5-REFINE-17 #17, P5-REFINE-18 #18, P5-REFINE-20 G3/G4) The count on a navigation entry: amber, because the entry it
 * sits on is the action; no badge at zero; the words for a screen reader inside. `n` comes from `GET /v1/attention/counts`
 * (C-1082), which is `null` for someone who cannot act there, so nobody else ever sees one.
 */
export function CountBadge({ n, testId, actionId }: { n: number | undefined | null; testId: string; actionId: string }) {
  if (!n || n <= 0) return null;
  return (
    <AttentionChip actionId={actionId} data-testid={testId} className="tabular-nums">
      <span aria-hidden="true">{n}</span>
      <span className="sr-only">{attentionWords(n)}</span>
    </AttentionChip>
  );
}
