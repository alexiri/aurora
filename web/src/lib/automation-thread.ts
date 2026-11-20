/**
 * (P5-REFINE-14, D-409) Every link to a run or a delivery goes to **that run's own conversation**:
 * `<agent page>?tab=chat&c=<conversation id>`. Runs, Deliveries and Run now all use this one helper. `null` when the run
 * has no thread (it was skipped, refused, or its thread was trimmed by the volume guard).
 */
export function automationThreadHref(agentBase: string, conversationId: string | null | undefined): string | null {
  return conversationId ? `${agentBase}?tab=chat&c=${encodeURIComponent(conversationId)}` : null;
}
