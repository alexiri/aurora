"use client";

import { useState } from "react";

import { Badge } from "@/components/ui/badge";
import { ChevronDownIcon, WrenchIcon } from "@/components/ui/icons";
import { formatDuration, toolGroupSummary, type ToolCall } from "@/lib/chat/layout";

const TONE = { ALLOW: "accent", HELD: "pending", DENY: "danger", TOOL: "neutral" } as const;

/**
 * A turn's consecutive tool calls folded into one row: "Used N tools · total duration". Open while the turn
 * runs and collapsed once it ends. A group holding a denied or failed call shows the DENY chip in its summary.
 */
export function ToolCallGroup({ calls, running }: { calls: ToolCall[]; running: boolean }) {
  const sum = toolGroupSummary(calls);
  const [open, setOpen] = useState(running);
  // Follow the turn: open while it runs, fold when it ends (the person can still toggle in between).
  const [seen, setSeen] = useState(running);
  if (seen !== running) {
    setSeen(running);
    setOpen(running);
  }
  return (
    <details
      open={open}
      onToggle={(e) => setOpen((e.currentTarget as HTMLDetailsElement).open)}
      className="group max-w-chat-card rounded-control border border-line bg-surface text-sm dark:bg-canvas"
    >
      <summary className="flex min-h-11 cursor-pointer list-none items-center gap-2 px-3 py-2 text-ink-2 md:min-h-9 [&::-webkit-details-marker]:hidden">
        <WrenchIcon aria-hidden="true" className="shrink-0" />
        <span className="min-w-0 flex-1 truncate">{sum.label}</span>
        {sum.hasDeny ? <Badge tone="danger">Deny</Badge> : null}
        <ChevronDownIcon aria-hidden="true" className="shrink-0 transition-transform group-open:rotate-180 motion-reduce:transition-none" />
      </summary>
      <ul className="divide-y divide-line border-t border-line">
        {calls.map((c) => (
          <li key={c.callId} className="flex min-w-0 items-start gap-2 px-3 py-2">
            <Badge tone={TONE[c.decision]} className="mt-0.5 shrink-0">
              {c.decision === "TOOL" && c.ok === false ? "Failed" : c.decision}
            </Badge>
            <span className="min-w-0 flex-1 break-words font-mono text-xs text-ink-2">
              <span className="font-medium text-ink">{c.name}</span> {c.summary}
              {c.result ? <span className="text-muted"> — {c.result}</span> : null}
            </span>
            <span className="shrink-0 text-xs tabular-nums text-muted">
              {c.ok === null ? "running…" : c.durationMs !== null ? formatDuration(c.durationMs) : c.ok ? "done" : "failed"}
            </span>
          </li>
        ))}
      </ul>
    </details>
  );
}
