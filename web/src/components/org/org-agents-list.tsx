"use client";

import Link from "next/link";

import { AgentAvatar } from "@/components/agent-avatar";
import { EmptyState, ErrorState, Loading } from "@/components/states";
import { Card } from "@/components/ui/card";
import { StatusDot } from "@/components/ui/status-dot";
import { useOrganizationAgents, type OrgAgent, type OrgAgentWorkspace } from "@/lib/api/org";
import { When } from "@/components/ui/when";

export interface OrgAgentRow {
  agent: OrgAgent;
  ws: OrgAgentWorkspace;
}
export interface OrgAgentGroup {
  key: string;
  title: string;
  personal: boolean;
  rows: OrgAgentRow[];
}

/** The group header: "ACME TEAM" for a team workspace, "BOB BUILDER (PERSONAL)" for a person's own. */
export function groupTitle(ws: Pick<OrgAgentWorkspace, "name" | "personal_of">): string {
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

/** (D-425) The state words of an agent's header, with their tone; Needs a key and Can't start are amber (they need a person). */
export function agentRowState(a: Pick<OrgAgent, "state">): { word: string; tone: "accent" | "pending" | "neutral" } {
  switch (a.state) {
    case "paused":
      return { word: "Paused", tone: "pending" };
    case "needs_key":
      return { word: "Needs a key", tone: "pending" };
    case "failed":
      return { word: "Can't start", tone: "pending" };
    case "working":
      return { word: "Working", tone: "accent" };
    case "starting":
      return { word: "Starting its computer…", tone: "neutral" };
    case "asleep":
      return { word: "Asleep", tone: "neutral" };
    default:
      return { word: "Ready", tone: "accent" };
  }
}

/** Every agent of the organization from the one organization-wide route (C-1090), with the header's state and last use. */
export function useOrgAgents(orgId: string | undefined) {
  const q = useOrganizationAgents(orgId);
  const rows: OrgAgentRow[] = (q.data?.items ?? []).map((a) => ({ agent: a, ws: a.workspace }));
  return {
    rows,
    groups: groupAgents(rows),
    pending: q.isPending,
    error: q.error ?? null,
    refetch: () => void q.refetch(),
  };
}

export function AgentRow({ row, orgSlug }: { row: OrgAgentRow; orgSlug: string }) {
  const state = agentRowState(row.agent);
  return (
    <li className="flex items-center gap-3 px-4 py-3">
      <AgentAvatar name={row.agent.name} url={null} tintKey={row.agent.id} size={36} />
      <div className="min-w-0 flex-1">
        <Link href={`/o/${orgSlug}/w/${row.ws.slug}/agents/${row.agent.handle}`} className="font-semibold text-ink underline-offset-2 hover:underline">
          {row.agent.name}
        </Link>
        <p className="truncate text-sm text-ink-2">
          {row.agent.last_active_at ? (
            <>
              used <When value={row.agent.last_active_at} />
            </>
          ) : (
            "never used"
          )}
        </p>
      </div>
      <StatusDot tone={state.tone}>{state.word}</StatusDot>
    </li>
  );
}

/** (org-6) Organization › Agents: one card, a header per workspace, a row per agent. */
export function OrgAgentsList({ orgId, orgSlug }: { orgId: string; orgSlug: string }) {
  const { groups, pending, error, refetch } = useOrgAgents(orgId);
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
    </div>
  );
}
