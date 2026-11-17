"use client";

import Link from "next/link";
import { useMemo } from "react";

import { Section } from "@/components/ui/section";
import { StatTile } from "@/components/ui/stat-tile";
import { useWorkspaceUsage } from "@/lib/api/usage";
import { wsRoutes } from "@/lib/routes";
import { compact, duration, exact, presetRange } from "@/lib/usage-format";

/**
 * (P5-USAGE-2, C-1004; `14 §Usage`) The agent's last 30 days on its Computer page: four values and **See usage**,
 * which opens Activity › Usage filtered to the agent. A card that cannot load says so in one line; it never blocks the page.
 */
export function UsageCard({ wsId, org, ws, agentId }: { wsId: string; org: string; ws: string; agentId: string }) {
  const { from, to } = useMemo(() => presetRange(30), []);
  const q = useWorkspaceUsage(wsId, from, to, agentId);
  const t = q.data?.totals;
  const tokens = t ? t.input_tokens + t.output_tokens : 0;
  return (
    <Section
      id="computer-usage"
      title="Usage"
      description="The last 30 days, in UTC."
      actions={
        <Link href={wsRoutes(org, ws).usage(agentId)} className="text-sm font-medium text-accent-ink underline-offset-2 hover:underline">
          See usage
        </Link>
      }
    >
      {q.isError ? (
        <p role="status" className="text-sm text-ink-2">
          Usage could not be loaded.
        </p>
      ) : (
        <div className="grid grid-cols-2 gap-3 lg:grid-cols-4">
          <StatTile label="Tokens" value={t ? compact(tokens) : "…"} title={t ? exact(tokens) : undefined} />
          <StatTile label="Turns" value={t ? compact(t.turns) : "…"} title={t ? exact(t.turns) : undefined} />
          <StatTile label="Turn time" value={t ? duration(t.turn_seconds) : "…"} />
          <StatTile label="Awake time" value={t ? duration(t.awake_seconds) : "…"} />
        </div>
      )}
    </Section>
  );
}
