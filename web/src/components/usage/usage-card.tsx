"use client";

import Link from "next/link";
import { useMemo } from "react";

import { Section } from "@/components/ui/section";
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
        // (P5-POLISH-6, C5) Two plain lines instead of four tiles.
        <div className="space-y-1 text-sm text-ink-2">
          <p>
            <span className="font-medium text-ink" title={t ? exact(t.turns) : undefined}>{t ? compact(t.turns) : "…"}</span> conversation {t?.turns === 1 ? "turn" : "turns"}
          </p>
          <p>
            <span className="font-medium text-ink" title={t ? exact(tokens) : undefined}>{t ? compact(tokens) : "…"}</span> tokens · awake{" "}
            <span className="font-medium text-ink">{t ? duration(t.awake_seconds) : "…"}</span>
          </p>
        </div>
      )}
    </Section>
  );
}
