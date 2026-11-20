"use client";

import Link from "next/link";
import { useQueries } from "@tanstack/react-query";
import { useState } from "react";

import { AgentAvatar } from "@/components/agent-avatar";
import { EmptyState, ErrorState, Loading } from "@/components/states";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { StatusDot } from "@/components/ui/status-dot";
import { api } from "@/lib/api/client";
import { retryUnlessClientError } from "@/lib/api/errors";
import { queryKeys, useWorkspaces, type Agent, type Workspace } from "@/lib/api/queries";
import { unwrap } from "@/lib/api/unwrap";

/** Workspaces read at once; more wait behind "Show the rest" (the fan-out is one request per workspace). */
export const AGENT_FAN_OUT = 50;

export interface OrgAgentRow {
  agent: Agent;
  ws: Workspace;
}
export interface OrgAgentGroup {
  key: string;
  title: string;
  personal: boolean;
  rows: OrgAgentRow[];
}

/** The group header: "ACME TEAM" for a team workspace, "BOB BUILDER (PERSONAL)" for a person's own. */
export function groupTitle(ws: Pick<Workspace, "name" | "personal_of">): string {
  return ws.personal_of ? `${ws.personal_of.name} (personal)` : ws.name;
}

/** Team workspaces first A to Z, then personal ones by owner name; workspaces without agents are left out. */
export function groupAgents(rows: OrgAgentRow[]): OrgAgentGroup[] {
  const byWs = new Map<string, OrgAgentGroup>();
  for (const r of rows) {
    const g = byWs.get(r.ws.id) ?? { key: r.ws.id, title: groupTitle(r.ws), personal: !!r.ws.personal_of, rows: [] };
    g.rows.push(r);
    byWs.set(r.ws.id, g);
  }
  return [...byWs.values()].sort((a, b) => Number(a.personal) - Number(b.personal) || a.title.localeCompare(b.title));
}

/**
 * The state word of an agent from what the list carries: **Paused** when it, its workspace or the organization is paused,
 * otherwise **Ready**. Working, Asleep and Needs a key come from the organization-wide agents route (C-1090).
 */
export function agentRowState(a: Pick<Agent, "effective_frozen">): { word: "Ready" | "Paused"; tone: "accent" | "pending" } {
  return a.effective_frozen ? { word: "Paused", tone: "pending" } : { word: "Ready", tone: "accent" };
}

/** Every agent of the organization, read workspace by workspace (there is no organization-wide route yet). */
export function useOrgAgents(orgId: string | undefined, limit: number = AGENT_FAN_OUT) {
  const workspaces = useWorkspaces(orgId);
  const all = workspaces.data?.items ?? [];
  const shown = all.slice(0, limit);
  const results = useQueries({
    queries: shown.map((w) => ({
      queryKey: queryKeys.agents(w.id),
      queryFn: () => unwrap(api.GET("/v1/workspaces/{ws}/agents", { params: { path: { ws: w.id } } })),
      retry: retryUnlessClientError,
    })),
  });
  const rows: OrgAgentRow[] = shown.flatMap((w, i) => (results[i]?.data?.items ?? []).map((agent) => ({ agent, ws: w })));
  return {
    rows,
    groups: groupAgents(rows),
    hidden: Math.max(0, all.length - shown.length),
    pending: workspaces.isPending || results.some((r) => r.isPending),
    error: workspaces.error ?? results.find((r) => r.error)?.error ?? null,
    refetch: () => {
      void workspaces.refetch();
      results.forEach((r) => void r.refetch());
    },
  };
}

export function AgentRow({ row, orgSlug }: { row: OrgAgentRow; orgSlug: string }) {
  const state = agentRowState(row.agent);
  return (
    <li className="flex items-center gap-3 px-4 py-3">
      <AgentAvatar name={row.agent.name} url={row.agent.avatar_url} tintKey={row.agent.id} size={36} />
      <div className="min-w-0 flex-1">
        <Link href={`/o/${orgSlug}/w/${row.ws.slug}/agents/${row.agent.handle}`} className="font-semibold text-ink underline-offset-2 hover:underline">
          {row.agent.name}
        </Link>
        {row.agent.brief ? <p className="truncate text-sm text-ink-2">{row.agent.brief}</p> : null}
      </div>
      <StatusDot tone={state.tone}>{state.word}</StatusDot>
    </li>
  );
}

/** (org-6) Organization › Agents: one card, a header per workspace, a row per agent. */
export function OrgAgentsList({ orgId, orgSlug }: { orgId: string; orgSlug: string }) {
  const [limit, setLimit] = useState(AGENT_FAN_OUT);
  const { groups, hidden, pending, error, refetch } = useOrgAgents(orgId, limit);
  if (error) return <ErrorState error={error} onRetry={refetch} />;
  if (pending && groups.length === 0) return <Loading variant="rows" label="Loading agents…" />;
  if (groups.length === 0) return <EmptyState title="No agents yet">Agents show up here once a workspace has one.</EmptyState>;
  return (
    <div className="space-y-3">
      <Card className="overflow-hidden p-0">
        {groups.map((g) => (
          <section key={g.key} aria-label={g.title}>
            <h3 className="border-b border-line bg-subtle px-4 py-2 text-label font-semibold uppercase tracking-label text-muted">{g.title}</h3>
            <ul className="divide-y divide-line" aria-label={`Agents in ${g.title}`}>
              {g.rows.map((r) => (
                <AgentRow key={r.agent.id} row={r} orgSlug={orgSlug} />
              ))}
            </ul>
          </section>
        ))}
      </Card>
      {hidden > 0 ? (
        <Button type="button" variant="outline" size="sm" onClick={() => setLimit((n) => n + AGENT_FAN_OUT)}>
          Show the rest ({hidden} more workspaces)
        </Button>
      ) : null}
    </div>
  );
}
