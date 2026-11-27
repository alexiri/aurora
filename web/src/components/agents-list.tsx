import type { Agent } from "@/lib/api/queries";

import { AgentCard } from "./agents/agent-card";
import { EmptyState } from "./states";
import { CardGrid } from "./ui/card-grid";

/**
 * Agents as cards in the shared grid (P5-LAYOUT-3, D-440). `showTokenBadges`: the viewer owns the workspace; stale /
 * expiring / expired token badges are shown to owners only (RFE-013, C-314). `wsId` and `orgId` add each agent's summary.
 */
export function AgentsList({
  agents,
  base,
  showTokenBadges = false,
  needs,
  wsId,
  orgId,
  chat = true,
}: {
  agents: readonly Agent[];
  base?: string;
  showTokenBadges?: boolean;
  /** (P5-REFINE-20 G5) Approvals waiting for the viewer, by agent id; only for those who can decide. No chip at zero. */
  needs?: Readonly<Record<string, number>>;
  wsId?: string;
  orgId?: string;
  /** **Chat** at the foot of each card; off for a viewer who only reads. */
  chat?: boolean;
}) {
  if (agents.length === 0) return <EmptyState title="No agents yet" />;
  return (
    <CardGrid aria-label="Agents">
      {agents.map((a) => (
        <AgentCard
          key={a.id}
          agent={a}
          href={base ? `${base}/${a.handle}` : undefined}
          needs={needs?.[a.id] ?? 0}
          showTokenBadges={showTokenBadges}
          wsId={wsId}
          orgId={orgId}
          chat={chat}
        />
      ))}
    </CardGrid>
  );
}
