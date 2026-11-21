"use client";

import { Card } from "@/components/ui/card";
import { whenInSentence } from "@/lib/format-date";
import { StatusDot } from "@/components/ui/status-dot";
import { useInstance } from "@/lib/api/queries";
import { useTerminators } from "@/lib/api/ssh";

/**
 * (P4-SSH-2, C-795, `docs/15 §SSH access`) The "SSH" line of the admin overview: the terminator replicas the API has seen
 * (heartbeat within 90 s) and the sessions open on them, shown only while SSH is enabled — nothing while loading, on
 * error, or when off. A deployment turns SSH on, then waits for this line to show a replica.
 */
export function SshLine() {
  const enabled = useInstance().data?.features.ssh === true;
  const terminators = useTerminators(enabled);
  if (!enabled || !terminators.data) return null;
  const live = terminators.data.filter((t) => t.live);
  const sessions = live.reduce((n, t) => n + t.sessions_open, 0);
  const last = terminators.data.map((t) => t.last_seen_at).sort().at(-1);
  return (
    <Card className="space-y-1 p-3 text-sm sm:p-3" data-testid="ssh-line">
      <div className="flex flex-wrap items-center gap-x-3 gap-y-1">
        <span className="font-medium text-ink">SSH</span>
        <StatusDot tone={live.length > 0 ? "accent" : "pending"}>{live.length > 0 ? "Terminator up" : "No terminator seen"}</StatusDot>
        <span className="min-w-0 break-words text-ink-2">
          {live.length === 0
            ? "No SSH terminator has reported in the last 90 seconds."
            : `${live.length} terminator ${live.length === 1 ? "replica" : "replicas"} live, ${sessions} open ${sessions === 1 ? "session" : "sessions"}` +
              (last ? `, last ${whenInSentence(last)}` : "")}
        </span>
      </div>
    </Card>
  );
}
