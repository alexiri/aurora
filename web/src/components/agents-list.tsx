import Link from "next/link";

import type { Agent } from "@/lib/api/queries";

import { AgentAvatar } from "./agent-avatar";
import { EmptyState } from "./states";
import { Badge } from "./ui/badge";
import { ItemList, ListRow } from "./ui/section";

export function AgentsList({ agents, base }: { agents: readonly Agent[]; base?: string }) {
  if (agents.length === 0) return <EmptyState title="No agents yet" />;
  return (
    <ItemList aria-label="Agents">
      {agents.map((a) => (
        <ListRow key={a.id}>
          <AgentAvatar name={a.name} url={a.avatar_url} tintKey={a.id} size={40} />
          <div className="flex min-w-0 flex-1 flex-wrap items-center gap-x-3 gap-y-1">
            {base ? (
              <Link href={`${base}/${a.handle}`} className="font-semibold text-ink underline-offset-2 hover:underline">
                {a.name}
              </Link>
            ) : (
              <span className="font-medium text-ink">{a.name}</span>
            )}
            <span className="font-mono text-sm text-muted">@{a.handle}</span>
            <Badge>{a.kind}</Badge>
            {a.is_default ? <Badge tone="accent">Default</Badge> : null}
            {a.byo?.posture === "disabled" ? (
              <Badge tone="danger" title="Its tokens are refused until the setting is loosened; nothing was deleted.">
                BYO disabled by {a.byo.posture_level}
              </Badge>
            ) : null}
          </div>
        </ListRow>
      ))}
    </ItemList>
  );
}
