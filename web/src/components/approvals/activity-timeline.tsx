"use client";

import Link from "next/link";
import { useEffect, useState } from "react";

import { Button } from "@/components/ui/button";
import { StatusDot } from "@/components/ui/status-dot";
import { When } from "@/components/ui/when";
import { useTimelineStream, type TimelineItem, type TimelineRequestEvent } from "@/lib/api/timeline";
import { ACTIVITY_OUTCOME, label, timelineOutcomeGroup, timelineWords } from "@/lib/labels";
import { formatDuration } from "@/lib/chat/layout";
import { timelineHref } from "@/lib/timeline-links";

/** Scrolled less than this far from the top counts as "at the top": new rows slide in; further down they wait. */
const NEAR_TOP_PX = 80;

/** Whether the page is scrolled to (near) the top, so a new row can appear without moving what the person is reading. */
export function useAtTop(): boolean {
  const [atTop, setAtTop] = useState(true);
  useEffect(() => {
    const on = () => setAtTop(window.scrollY <= NEAR_TOP_PX);
    on();
    window.addEventListener("scroll", on, { passive: true });
    return () => window.removeEventListener("scroll", on);
  }, []);
  return atTop;
}

/** What one live stream reports to the page: rows waiting ("3 new · Show") and the way to show them. */
export interface LiveState {
  pending: number;
  show: (() => void) | null;
}

/**
 * One agent's timeline stream, drawn as nothing: its `row` upserts merge into the cached feeds (the sentence list), its
 * `request` events (`technical`) tell the Technical view to read again. Mounted once per live agent.
 */
export function TimelineLive({
  agentId,
  atTop,
  technical,
  onRequest,
  report,
}: {
  agentId: string;
  atTop: boolean;
  technical: boolean;
  onRequest: (e: TimelineRequestEvent) => void;
  report: (agentId: string, s: LiveState) => void;
}) {
  const s = useTimelineStream(agentId, { atTop, technical, onRequest });
  useEffect(() => {
    report(agentId, { pending: s.pendingNew, show: s.showNew });
    return () => report(agentId, { pending: 0, show: null });
  }, [agentId, s.pendingNew, s.showNew, report]);
  return null;
}

/** "Waiting for you in Approvals" is the one meta line that is a link. */
const waitingForYou = (i: TimelineItem) => i.kind === "asked_before" && i.state === "waiting";

/** The row's recorded fields, as the API returned them (the request path is never on the timeline, SEC-143). */
export function recordedFields(i: TimelineItem): string[] {
  const out = [`kind ${i.kind}`, `state ${i.state}`];
  if (i.outcome) out.push(`outcome ${i.outcome}`);
  if (i.model) out.push(`model ${i.model}`);
  if (i.tool) out.push(`tool ${i.tool}`);
  if (i.host) out.push(`host ${i.host}`);
  if (i.count != null) out.push(`count ${i.count}`);
  if (i.duration_ms != null) out.push(`took ${formatDuration(i.duration_ms)}`);
  if (i.command) out.push(`command ${i.command}`);
  if (i.files?.length) out.push(`files ${i.files.join(", ")}`);
  out.push(`id ${i.id}`);
  return out;
}

/** The sentence rows of Activity (workspace-2): dot, sentence, meta line, time, Details. */
export function ActivityRows({ items, base, agentBase }: { items: TimelineItem[]; base: string; agentBase: (agentId: string) => string }) {
  const [open, setOpen] = useState<string | null>(null);
  return (
    <ul className="divide-y divide-line overflow-hidden rounded-card border border-line bg-surface shadow-card" data-testid="activity-sentences">
      {items.map((i) => {
        const w = timelineWords(i);
        const href = timelineHref(i, { base, agentBase: agentBase(i.agent.id) });
        return (
          <li key={i.id} className="space-y-2 px-4 py-3">
            <div className="flex flex-wrap items-center gap-x-3 gap-y-1">
              <StatusDot tone={w.tone}>
                <span className="sr-only">{label(ACTIVITY_OUTCOME, timelineOutcomeGroup(i))}</span>
              </StatusDot>
              <div className="min-w-0 flex-1 basis-56">
                <p className="text-sm font-medium text-ink">{w.title}</p>
                <p className="text-xs text-muted">
                  {waitingForYou(i) ? (
                    <Link className="text-link underline" href={`${base}/approvals`}>
                      {w.meta}
                    </Link>
                  ) : (
                    w.meta
                  )}
                </p>
              </div>
              <When value={i.at} className="text-sm text-muted" />
              <Button size="sm" variant="ghost" aria-expanded={open === i.id} onClick={() => setOpen(open === i.id ? null : i.id)}>
                Details
              </Button>
            </div>
            {open === i.id ? (
              <div className="rounded-control bg-canvas p-3" data-testid="activity-raw">
                <p className="text-xs font-medium uppercase tracking-wide text-muted">Details · what was recorded</p>
                <p className="mt-1 break-words font-mono text-xs text-ink-2">{recordedFields(i).join(" · ")}</p>
                {href ? (
                  <Link className="mt-2 inline-block text-sm text-link underline" href={href}>
                    Open
                  </Link>
                ) : null}
              </div>
            ) : null}
          </li>
        );
      })}
    </ul>
  );
}
