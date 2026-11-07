import Link from "next/link";

import type { Agent } from "@/lib/api/queries";

import { AgentAvatar } from "./agent-avatar";
import { EmptyState } from "./states";
import { Badge } from "./ui/badge";

export function AgentsList({ agents, base }: { agents: readonly Agent[]; base?: string }) {
  if (agents.length === 0) return <EmptyState title="No agents yet" />;
  return (
    <ul className="divide-y divide-line overflow-hidden rounded-card border border-line bg-surface" aria-label="Agents">
      {agents.map((a) => (
        <li key={a.id} className="flex flex-wrap items-center gap-x-3 gap-y-1 p-3">
          <AgentAvatar name={a.name} url={a.avatar_url} tintKey={a.id} />
          {base ? (
            <Link href={`${base}/${a.handle}`} className="font-semibold underline-offset-2 hover:underline">
              {a.name}
            </Link>
          ) : (
            <span className="font-medium">{a.name}</span>
          )}
          <span className="font-mono text-sm text-muted">@{a.handle}</span>
          <Badge>{a.kind}</Badge>
          {a.is_default ? <Badge tone="accent">Default</Badge> : null}
          {a.byo?.posture === "disabled" ? (
            <Badge tone="danger" title="Its tokens are refused until the setting is loosened; nothing was deleted.">
              BYO disabled by {a.byo.posture_level}
            </Badge>
          ) : null}
        </li>
      ))}
    </ul>
  );
}
