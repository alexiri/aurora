"use client";

import Link from "next/link";
import { useParams } from "next/navigation";

import { Avatar } from "@/components/agent-avatar";
import { AgentSummary } from "@/components/agents/agent-summary";
import { CreateAgentDialog } from "@/components/agents/create-agent-dialog";
import { decisionTone } from "@/components/approvals/activity-view";
import { ErrorState, Loading } from "@/components/states";
import { Badge } from "@/components/ui/badge";
import { GettingStarted } from "@/components/onboarding/getting-started";
import { NeedsYou } from "@/components/overview/needs-you";
import { PageHeader } from "@/components/ui/page-header";
import { Section } from "@/components/ui/section";
import { StatusDot } from "@/components/ui/status-dot";
import { useActivity, useApprovalsStream, useCanApprove, useNow } from "@/lib/api/approvals";
import { useAgents, useResolve, useWorkspace } from "@/lib/api/queries";

function ago(iso: string, now: number): string {
  const s = Math.max(0, Math.floor((now - new Date(iso).getTime()) / 1000));
  if (s < 60) return "just now";
  const m = Math.floor(s / 60);
  if (m < 60) return `${m} min ago`;
  const h = Math.floor(m / 60);
  return h < 24 ? `${h} h ago` : `${Math.floor(h / 24)} d ago`;
}

function LiveIndicator({ wsId }: { wsId: string }) {
  const { live } = useApprovalsStream(wsId);
  return <StatusDot tone={live ? "accent" : "pending"}>{live ? "Live" : "Reconnecting…"}</StatusDot>;
}

export function OverviewView() {
  const { org, ws } = useParams<{ org: string; ws: string }>();
  const resolved = useResolve(org, ws);
  const workspace = resolved.data?.workspace;

  if (resolved.isPending) return <Loading />;
  if (resolved.isError) return <ErrorState error={resolved.error} onRetry={() => resolved.refetch()} />;
  if (!workspace) {
    return <ErrorState error={{ code: "not_found" }} />;
  }
  return (
    <OverviewBody
      org={org}
      ws={ws}
      wsId={workspace.id}
      orgId={resolved.data.org.id}
      name={workspace.name}
    />
  );
}

function OverviewBody({
  org,
  ws,
  wsId,
  orgId,
  name,
}: {
  org: string;
  ws: string;
  wsId: string;
  orgId: string;
  name: string;
}) {
  const agents = useAgents(wsId);
  const workspace = useWorkspace(wsId);
  const canApprove = useCanApprove(wsId);
  const activity = useActivity(wsId, { agent: "", decision: "", host: "" });
  const now = useNow(30_000);
  const base = `/o/${org}/w/${ws}`;
  const recent = (activity.data?.pages[0]?.items ?? []).slice(0, 6);
  const byId = new Map(agents.data?.items.map((a) => [a.id, a]) ?? []);

  return (
    <div className="flex flex-col gap-8">
      <PageHeader title={name} description="Your agents, what they asked for and what they did." className="mb-0" />

      <NeedsYou wsId={wsId} base={base} />

      <Section id="agents" title="Agents">
        {agents.isPending ? <Loading /> : null}
        {agents.isError ? <ErrorState error={agents.error} onRetry={() => agents.refetch()} /> : null}
        {agents.data ? (
          <ul aria-label="Agents" className="grid gap-3 sm:grid-cols-2 lg:grid-cols-3">
            {agents.data.items.map((a) => (
              <li key={a.id} className="flex min-w-0 flex-col rounded-card border border-line bg-surface shadow-card hover:border-line-strong">
                <Link href={`${base}/agents/${a.handle}`} className="flex min-w-0 items-start gap-3 p-4">
                  <Avatar name={a.name} url={a.avatar_url} tintKey={a.id} size={40} />
                  <span className="min-w-0 flex-1">
                    <span className="block truncate font-semibold text-ink">{a.name}</span>
                    <span className="block truncate font-mono text-xs text-muted">@{a.handle}</span>
                    <span className="mt-2 flex flex-wrap gap-1.5">
                      <Badge>{a.kind}</Badge>
                      {a.is_default ? <Badge tone="accent">Default</Badge> : null}
                      {a.byo?.posture === "disabled" ? <Badge tone="danger">BYO disabled by {a.byo.posture_level}</Badge> : null}
                    </span>
                  </span>
                </Link>
                {/* The agent's summary (C-986): the same sentences as Welcome and the Access tab. */}
                {workspace.data ? (
                  <AgentSummary
                    wsId={wsId}
                    orgId={orgId}
                    agent={a}
                    canOwn={workspace.data.my_role === "owner"}
                    className="space-y-1 border-t border-line px-4 py-3 text-sm text-ink-2"
                  />
                ) : null}
              </li>
            ))}
            <li className="flex min-h-22 items-center justify-center rounded-card border border-dashed border-line-strong bg-surface p-4">
              <CreateAgentDialog wsId={wsId} />
            </li>
          </ul>
        ) : null}
      </Section>

      <div className="grid gap-8 lg:grid-cols-2">
        <Section id="first-run" title="Getting started">
          {workspace.data ? (
            <GettingStarted org={org} ws={ws} wsId={wsId} orgId={orgId} isOwner={workspace.data.my_role === "owner"} variant="overview" />
          ) : workspace.isError ? (
            <ErrorState error={workspace.error} onRetry={() => void workspace.refetch()} />
          ) : (
            <Loading />
          )}
        </Section>
        <Section id="activity" title="Recent activity" actions={canApprove ? <LiveIndicator wsId={wsId} /> : null}>
          {activity.isPending ? <Loading /> : null}
          {activity.isError ? <ErrorState error={activity.error} onRetry={() => activity.refetch()} /> : null}
          {activity.data && recent.length === 0 ? <p className="text-sm text-muted">No activity yet.</p> : null}
          {recent.length > 0 ? (
            <ul aria-label="Recent activity" className="divide-y divide-line overflow-hidden rounded-card border border-line bg-surface shadow-card">
              {recent.map((e) => (
                <li key={e.id} className="space-y-1 px-4 py-3 text-sm">
                  <div className="flex flex-wrap items-center gap-2">
                    <Badge tone={decisionTone(e.decision)}>{e.decision}</Badge>
                    <span className="ml-auto text-xs text-muted">{ago(e.ts, now)}</span>
                  </div>
                  <p className="break-all font-mono text-xs text-ink-2">
                    {e.method} {e.host}
                    {e.path}
                  </p>
                  <p className="text-xs text-muted">{byId.get(e.agent_id)?.name ?? "—"}</p>
                </li>
              ))}
            </ul>
          ) : null}
          <Link href={`${base}/activity`} className="inline-block text-sm text-link underline">
            View all activity
          </Link>
        </Section>
      </div>
    </div>
  );
}
