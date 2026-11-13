"use client";

import { fmtDate } from "@/components/account/key-table";
import { EmptyState, ErrorState, Loading } from "@/components/states";
import { Badge } from "@/components/ui/badge";
import { inputClass } from "@/components/ui/field";
import { Notice } from "@/components/ui/notice";
import { ItemList, ListRow, Section } from "@/components/ui/section";
import { ApiError } from "@/lib/api/client";
import { errorMessage } from "@/lib/api/errors";
import { LINK_POLICIES, useAgentLinks, usePeerTasks, useSetLinkPolicy, type AgentLink, type LinkPolicy } from "@/lib/api/peers";

function LinkRow({ agentId, link }: { agentId: string; link: AgentLink }) {
  const set = useSetLinkPolicy(agentId);
  const stale = set.error instanceof ApiError && set.error.status === 409;
  return (
    <li className="space-y-2 p-3 sm:px-4">
      <div className="flex flex-wrap items-baseline gap-x-3 gap-y-1">
        <span className="font-medium text-ink">{link.peer.name}</span>
        <span className="font-mono text-sm text-muted">@{link.peer.handle}</span>
        <span className="text-xs text-muted sm:ml-auto">Their policy: {link.peer.policy}</span>
      </div>
      <div className="flex flex-wrap items-center gap-x-3 gap-y-1 text-sm">
        <label htmlFor={`pol-${link.id}`} className="font-medium text-ink">
          My policy for {link.peer.name}
        </label>
        <select
          id={`pol-${link.id}`}
          className={`${inputClass} max-w-40`}
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
        <Notice tone="danger" role="alert">
          {stale ? "This link was changed by someone else. The latest version has been loaded; try again." : errorMessage(set.error)}
        </Notice>
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
    <ItemList aria-label="Agent links">
      {q.data.items.map((l) => (
        <LinkRow key={l.id} agentId={agentId} link={l} />
      ))}
    </ItemList>
  );
}

function taskTone(status: string): "accent" | "pending" | "danger" | "neutral" {
  if (status === "completed") return "accent";
  if (status === "cancelled" || status === "expired") return "danger";
  if (status === "open" || status === "queued") return "pending";
  return "neutral";
}

function Tasks({ agentId }: { agentId: string }) {
  const q = usePeerTasks(agentId);
  if (q.isPending) return <Loading />;
  if (q.isError) return <ErrorState error={q.error} onRetry={() => void q.refetch()} />;
  if (q.data.items.length === 0) return <EmptyState title="No peer tasks" />;
  return (
    <ItemList aria-label="Peer tasks">
      {q.data.items.map((t) => (
        <ListRow key={t.id}>
          <div className="min-w-0 flex-1 space-y-1 text-sm">
            <div className="flex flex-wrap items-center gap-x-3 gap-y-1">
              <span className="font-medium text-ink">{t.direction === "outgoing" ? "Outgoing" : "Incoming"}</span>
              <Badge tone={taskTone(t.status)}>{t.status}</Badge>
              <span className="text-muted">
                Budget {t.messages_used}/{t.message_budget}
              </span>
              <span className="text-xs text-muted sm:ml-auto">{fmtDate(t.created_at)}</span>
            </div>
            {t.requester ? <p className="text-xs text-muted">Requested by {t.requester.display_name}</p> : null}
            {t.report ? <p className="whitespace-pre-wrap text-ink-2">{t.report}</p> : null}
          </div>
        </ListRow>
      ))}
    </ItemList>
  );
}

export function PeersTab({ agentId }: { agentId: string }) {
  return (
    <div className="space-y-8">
      <Section id="agent-links" title="Agent links" description="Other agents this agent may hand tasks to or accept tasks from.">
        <Links agentId={agentId} />
      </Section>
      <Section id="peer-tasks" title="Peer tasks">
        <Tasks agentId={agentId} />
      </Section>
    </div>
  );
}
