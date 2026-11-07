import Link from "next/link";

import type { Agent } from "@/lib/api/queries";

import { AgentAvatar } from "./agent-avatar";
import { EmptyState } from "./states";

export function AgentsList({ agents, base }: { agents: readonly Agent[]; base?: string }) {
  if (agents.length === 0) return <EmptyState title="No agents yet" />;
  return (
    <ul className="divide-y divide-neutral-200 rounded-md border border-neutral-200" aria-label="Agents">
      {agents.map((a) => (
        <li key={a.id} className="flex flex-wrap items-center gap-x-3 gap-y-1 p-3">
          <AgentAvatar name={a.name} url={a.avatar_url} />
          {base ? (
            <Link href={`${base}/${a.handle}`} className="font-medium underline-offset-2 hover:underline">
              {a.name}
            </Link>
          ) : (
            <span className="font-medium">{a.name}</span>
          )}
          <span className="font-mono text-sm text-neutral-600">@{a.handle}</span>
          <span className="rounded bg-neutral-100 px-1.5 py-0.5 text-xs uppercase">{a.kind}</span>
          {a.is_default ? (
            <span className="rounded bg-emerald-100 px-1.5 py-0.5 text-xs text-emerald-800">
              Default
            </span>
          ) : null}
        </li>
      ))}
    </ul>
  );
}
