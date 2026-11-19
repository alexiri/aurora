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
import { useWorkspaceAttention } from "@/components/accounts/attention";
import { buttonVariants } from "@/components/ui/button";
import { useMeOnboarding } from "@/lib/api/onboarding";
import { useActivity, useApprovalsStream, useCanApprove, useNow, usePendingApprovals } from "@/lib/api/approvals";
import { useAgents, useResolve, useWorkspace } from "@/lib/api/queries";
import { ago } from "@/lib/utils";

/** "2 things need you." / "Daneel is ready. Nothing needs you." (the Overview board's sentence, from real state). */
export function overviewSentence(needs: number, agentNames: string[]): string {
  if (needs > 0) return needs === 1 ? "1 thing needs you." : `${needs} things need you.`;
  if (agentNames.length === 0) return "No agents yet. Add one to get started.";
  return `${agentNames.length === 1 ? `${agentNames[0]} is ready.` : "Your agents are ready."} Nothing needs you.`;
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

  // (P5-POLISH-7, D5) The sentence comes from state: what needs the person, else that the agent is ready.
  const pendingApprovals = usePendingApprovals(wsId, canApprove);
  const broken = useWorkspaceAttention(wsId);
  const onboarding = useMeOnboarding();
  const needs = (canApprove ? (pendingApprovals.data?.items?.length ?? 0) : 0) + broken.length;
  const list = agents.data?.items ?? [];
  const sentence = overviewSentence(needs, list.map((a) => a.name));
  // The guide shows here only after Welcome has stopped being the landing page (it is seen), so never on both pages.
  const guideHere = onboarding.data?.welcome_seen === true;
  void name;

  return (
    <div className="flex flex-col gap-8">
      <PageHeader title="Overview" description={sentence} className="mb-0" />

      <NeedsYou wsId={wsId} base={base} orgId={orgId} />

      <Section id="agents" title="Agents" actions={<CreateAgentDialog wsId={wsId} />}>
        {agents.isPending ? <Loading /> : null}
        {agents.isError ? <ErrorState error={agents.error} onRetry={() => agents.refetch()} /> : null}
        {agents.data ? (
          <ul aria-label="Agents" className="divide-y divide-line overflow-hidden rounded-card border border-line bg-surface shadow-card">
            {list.map((a) => (
              <li key={a.id} className="flex min-w-0 flex-wrap items-center gap-x-4 gap-y-2 px-4 py-3">
                <Link href={`${base}/agents/${a.handle}`} className="flex min-w-0 flex-1 basis-60 items-center gap-3">
                  <Avatar name={a.name} url={a.avatar_url} tintKey={a.id} size={40} />
                  <span className="min-w-0 flex-1">
                    <span className="block truncate font-semibold text-ink">{a.name}</span>
                    {/* The agent's summary (C-986): the same sentences as Welcome and the Access page. */}
                    <AgentSummary wsId={wsId} orgId={orgId} agent={a} className="space-y-0.5 text-sm text-ink-2" />
                    {a.byo?.posture === "disabled" ? <Badge tone="danger">BYO disabled by {a.byo.posture_level}</Badge> : null}
                  </span>
                </Link>
                <Link href={`${base}/agents/${a.handle}`} className={buttonVariants({ variant: "outline", size: "sm" })} aria-label={`Chat with ${a.name}`}>
                  Chat
                </Link>
              </li>
            ))}
          </ul>
        ) : null}
      </Section>

      <div className={guideHere ? "grid gap-8 lg:grid-cols-2" : "grid gap-8"}>
        {guideHere ? (
          <Section id="first-run" title="Getting started">
            {workspace.data ? (
              <GettingStarted org={org} ws={ws} wsId={wsId} orgId={orgId} isOwner={workspace.data.my_role === "owner"} variant="overview" />
            ) : workspace.isError ? (
              <ErrorState error={workspace.error} onRetry={() => void workspace.refetch()} />
            ) : (
              <Loading />
            )}
          </Section>
        ) : null}
        <Section id="activity" title={canApprove ? <span className="flex flex-wrap items-center gap-3">Recent activity <LiveIndicator wsId={wsId} /></span> : "Recent activity"}>
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
