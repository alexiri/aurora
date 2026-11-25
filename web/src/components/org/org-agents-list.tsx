"use client";

import Link from "next/link";

import { AgentAvatar } from "@/components/agent-avatar";
import { EmptyState, ErrorState, Loading } from "@/components/states";
import { Card } from "@/components/ui/card";
import { Button } from "@/components/ui/button";
import { StatusDot } from "@/components/ui/status-dot";
import { FreezeControl } from "@/components/freeze/freeze-panel";
import { useFreezeAgent, useFreezeAuthority } from "@/lib/api/freeze";
import { useOrganizationAgents, type OrgAgent, type OrgAgentWorkspace } from "@/lib/api/org";
import { useWorkspaces } from "@/lib/api/queries";
import { When } from "@/components/ui/when";
import { orgRoutes } from "@/lib/routes";
import { personName } from "@/lib/workspace-name";

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
  return ws.personal_of ? `${personName(ws.personal_of.name)} (personal)` : ws.name;
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

/** The row action for an agent of a workspace the viewer manages but is not in (FIX-235, D-477):
 * Pause opens the freeze dialog; Resume lifts the agent's own pause at once (a workspace-level
 * pause is lifted on the manage page, where the workspace's controls are). */
function OrgAgentAction({ agentId, name, orgId, paused }: { agentId: string; name: string; orgId: string; paused: boolean }) {
  const m = useFreezeAgent(agentId);
  const rank = useFreezeAuthority(orgId);
  if (paused) {
    return (
      <Button variant="outline" size="sm" disabled={m.unfreeze.isPending} onClick={() => void m.unfreeze.mutateAsync()} aria-label={`Resume ${name}`}>
        Resume
      </Button>
    );
  }
  return <FreezeControl subject="agent" name={name} frozen={null} callerRank={rank} freeze={m.freeze} unfreeze={m.unfreeze} buttonLabel="Pause…" buttonVariant="outline" />;
}

export function AgentRow({ row, orgSlug, orgId, inWs = true }: { row: OrgAgentRow; orgSlug: string; orgId: string; /** The viewer holds own access to the agent's workspace (FIX-235); only then is the agent a link. */ inWs?: boolean }) {
  const state = agentRowState(row.agent);
  return (
    <li className="flex items-center gap-3 px-4 py-3">
      <AgentAvatar name={row.agent.name} url={null} tintKey={row.agent.id} size={36} />
      <div className="min-w-0 flex-1">
        {inWs ? (
          <Link href={`/o/${orgSlug}/w/${row.ws.slug}/agents/${row.agent.handle}`} className="font-semibold text-ink underline-offset-2 hover:underline">
            {row.agent.name}
          </Link>
        ) : (
          <span className="font-semibold text-ink">{row.agent.name}</span>
        )}
        <p className="truncate text-sm text-ink-2">
          {inWs ? (
            row.agent.last_active_at ? (
              <>
                used <When value={row.agent.last_active_at} />
              </>
            ) : (
              "never used"
            )
          ) : (
            <Link href={orgRoutes(orgSlug).workspace(row.ws.slug)} className="underline underline-offset-2 hover:underline" data-testid="agent-manage-link">
              {row.ws.name}
            </Link>
          )}
        </p>
      </div>
      <StatusDot tone={state.tone}>{state.word}</StatusDot>
      {inWs ? null : <OrgAgentAction agentId={row.agent.id} name={row.agent.name} orgId={orgId} paused={row.agent.state === "paused"} />}
    </li>
  );
}

/** (org-6) Organization › Agents: one card, a header per workspace, a row per agent. */
export function OrgAgentsList({ orgId, orgSlug }: { orgId: string; orgSlug: string }) {
  const { groups, pending, error, refetch } = useOrgAgents(orgId);
  // (FIX-235, D-477) An agent links only when the viewer is one of its workspace's people; the
  // org agents route's rows carry no my_role, so join the org's workspace list (C-1160).
  const workspaces = useWorkspaces(orgId);
  const inWs = new Map((workspaces.data?.items ?? []).filter((w) => w.my_role != null).map((w) => [w.id, true]));
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
                <AgentRow key={r.agent.id} row={r} orgSlug={orgSlug} orgId={orgId} inWs={inWs.has(r.ws.id)} />
              ))}
            </ul>
          </section>
        ))}
      </Card>
    </div>
  );
}
