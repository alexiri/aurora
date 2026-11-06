"use client";

import { fmtDate } from "@/components/account/key-table";
import { EmptyState, ErrorState, Loading } from "@/components/states";
import { inputClass } from "@/components/ui/field";
import { ApiError } from "@/lib/api/client";
import { errorMessage } from "@/lib/api/errors";
import { LINK_POLICIES, useAgentLinks, usePeerTasks, useSetLinkPolicy, type AgentLink, type LinkPolicy } from "@/lib/api/peers";

function LinkRow({ agentId, link }: { agentId: string; link: AgentLink }) {
  const set = useSetLinkPolicy(agentId);
  const stale = set.error instanceof ApiError && set.error.status === 409;
  return (
    <li className="space-y-1 rounded-md border border-neutral-200 p-4">
      <div className="flex flex-wrap items-baseline gap-2">
        <span className="font-medium">{link.peer.name}</span>
        <span className="font-mono text-sm text-neutral-600">@{link.peer.handle}</span>
        <span className="ml-auto text-xs text-neutral-600">Their policy: {link.peer.policy}</span>
      </div>
      <div className="flex items-center gap-2 text-sm">
        <label htmlFor={`pol-${link.id}`}>My policy for {link.peer.name}</label>
        <select
          id={`pol-${link.id}`}
          className={`${inputClass} max-w-[10rem]`}
          value={link.self.policy}
          disabled={set.isPending}
          onChange={(e) => set.mutate({ link, policy: e.target.value as LinkPolicy })}
        >
          {LINK_POLICIES.map((p) => (
            <option key={p} value={p}>
              {p}
            </option>
          ))}
        </select>
      </div>
      {set.error ? (
        <p role="alert" className="text-sm text-red-700">
          {stale ? "This link was changed by someone else. The latest version has been loaded; try again." : errorMessage(set.error)}
        </p>
      ) : null}
    </li>
  );
}

function Links({ agentId }: { agentId: string }) {
  const q = useAgentLinks(agentId);
  if (q.isPending) return <Loading />;
  if (q.isError) return <ErrorState error={q.error} onRetry={() => void q.refetch()} />;
  if (q.data.items.length === 0) return <EmptyState title="No agent links">Links let this agent hand tasks to other agents.</EmptyState>;
  return (
    <ul className="space-y-2">
      {q.data.items.map((l) => (
        <LinkRow key={l.id} agentId={agentId} link={l} />
      ))}
    </ul>
  );
}

function Tasks({ agentId }: { agentId: string }) {
  const q = usePeerTasks(agentId);
  if (q.isPending) return <Loading />;
  if (q.isError) return <ErrorState error={q.error} onRetry={() => void q.refetch()} />;
  if (q.data.items.length === 0) return <EmptyState title="No peer tasks" />;
  return (
    <ul className="space-y-2">
      {q.data.items.map((t) => (
        <li key={t.id} className="space-y-1 rounded-md border border-neutral-200 p-4 text-sm">
          <div className="flex flex-wrap gap-3">
            <span className="font-medium">{t.direction === "outgoing" ? "Outgoing" : "Incoming"}</span>
            <span>{t.status}</span>
            <span className="text-neutral-600">
              Budget {t.messages_used}/{t.message_budget}
            </span>
            <span className="ml-auto text-xs text-neutral-600">{fmtDate(t.created_at)}</span>
          </div>
          {t.requester ? <p className="text-xs text-neutral-600">Requested by {t.requester.display_name}</p> : null}
          {t.report ? <p className="whitespace-pre-wrap">{t.report}</p> : null}
        </li>
      ))}
    </ul>
  );
}

export function PeersTab({ agentId }: { agentId: string }) {
  return (
    <div className="space-y-6">
      <section aria-label="Agent links" className="space-y-2">
        <h2 className="font-medium">Agent links</h2>
        <Links agentId={agentId} />
      </section>
      <section aria-label="Peer tasks" className="space-y-2">
        <h2 className="font-medium">Peer tasks</h2>
        <Tasks agentId={agentId} />
      </section>
    </div>
  );
}
