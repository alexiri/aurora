import Link from "next/link";

import type { Agent } from "@/lib/api/queries";

import { AgentAvatar } from "./agent-avatar";
import { TokenBadges } from "./agents/token-badges";
import { EmptyState } from "./states";
import { AttentionChip } from "./ui/attention-chip";
import { agentChip as chipOf } from "@/lib/agent-chip";

import { Badge } from "./ui/badge";
import { ItemList, ListRow } from "./ui/section";

/**
 * `showTokenBadges`: the viewer owns the workspace; stale / expiring / expired token badges are
 * shown to owners only (RFE-013, C-314).
 */
export function AgentsList({
  agents,
  base,
  showTokenBadges = false,
  needs,
}: {
  agents: readonly Agent[];
  base?: string;
  showTokenBadges?: boolean;
  /** (P5-REFINE-20 G5) Approvals waiting for the viewer, by agent id; only for those who can decide. No chip at zero. */
  needs?: Readonly<Record<string, number>>;
}) {
  if (agents.length === 0) return <EmptyState title="No agents yet" />;
  return (
    <ItemList aria-label="Agents">
      {agents.map((a) => (
        <ListRow key={a.id}>
          <AgentAvatar name={a.name} url={a.avatar_url} tintKey={a.id} size={40} />
          <div className="flex min-w-0 flex-1 flex-wrap items-center gap-x-3 gap-y-1">
            {base ? (
              <Link id={`agent-link-${a.id}`} href={`${base}/${a.handle}`} className="font-semibold text-ink underline-offset-2 hover:underline">
                {a.name}
              </Link>
            ) : (
              <span className="font-medium text-ink">{a.name}</span>
            )}
            <span className="font-mono text-sm text-muted">@{a.handle}</span>
            {base && (needs?.[a.id] ?? 0) > 0 ? (
              <AttentionChip actionId={`agent-link-${a.id}`} data-testid="agent-needs-count" role="img" aria-label={`${needs?.[a.id]} ${needs?.[a.id] === 1 ? "approval" : "approvals"} waiting for you`}>
                <span aria-hidden="true">{needs?.[a.id]}</span>
              </AttentionChip>
            ) : null}
            {chipOf(a) ? <Badge tone={chipOf(a)!.tone}>{chipOf(a)!.text}</Badge> : null}
            {a.byo?.posture === "disabled" ? (
              <Badge tone="danger" title="Its tokens are refused until the setting is loosened; nothing was deleted.">
                BYO disabled by {a.byo.posture_level}
              </Badge>
            ) : null}
            {showTokenBadges ? <TokenBadges token={a.byo?.token} /> : null}
          </div>
        </ListRow>
      ))}
    </ItemList>
  );
}
