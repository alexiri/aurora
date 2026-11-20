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
import { useWorkspaceCounts } from "@/lib/api/attention";
import { buttonVariants } from "@/components/ui/button";
import { useMeOnboarding } from "@/lib/api/onboarding";
import { useActivity, useApprovalsStream, useCanApprove, useNow } from "@/lib/api/approvals";
import { actionsOnly } from "@/components/agents/status-rail/status-data";
import { useAccounts } from "@/lib/api/accounts";
import { useOrgSetupRequests } from "@/lib/api/app-setup-requests";
import { useAgents, useOrg, useResolve, useWorkspace } from "@/lib/api/queries";
import { ago } from "@/lib/utils";

/** "2 things need you." / "Daneel is ready. Nothing needs you." (the Overview board's sentence, from real state). */
export function overviewSentence(needs: number, agentNames: string[]): string {
  if (needs > 0) return needs === 1 ? "1 thing needs you." : `${needs} things need you.`;
  if (agentNames.length === 0) return "No agents yet. Add one to get started.";
  return `${agentNames.length === 1 ? `${agentNames[0]} is ready.` : "Your agents are ready."} Nothing needs you.`;
}

/**
 * (P5-POLISH-11, D5) One activity row in words: "Daneel used create issue in Team tracker." / "Daneel reached
 * api.example.com." — never `POST host/path`. A call that was not let through reads "tried to".
 */
export function activityLine(
  e: { decision: string; tool_id?: string | null; host: string },
  agent: string,
  account?: string,
): string {
  const let_through = e.decision === "allow" || e.decision === "approved";
  const verb = let_through ? "" : "tried to ";
  if (e.tool_id) {
    const tool = e.tool_id.replace(/[._-]+/g, " ").trim();
    return `${agent} ${verb}${let_through ? "used" : "use"} ${tool}${account ? ` in ${account}` : ""}.`;
  }
  return `${agent} ${verb}${let_through ? "reached" : "reach"} ${e.host}.`;
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
  // The agent's own model calls are thinking, not an action (as in the status rail): not listed.
  const recent = actionsOnly(activity.data?.pages[0]?.items ?? []).slice(0, 6);
  const accountNames = new Map((useAccounts({ kind: "workspace", id: wsId }).data?.items ?? []).map((a) => [a.ref.id, a.name]));
  const byId = new Map(agents.data?.items.map((a) => [a.id, a]) ?? []);

  // (P5-POLISH-7, D5) The sentence comes from state: what needs the person, else that the agent is ready.
  // (P5-REFINE-16 item 19) The workspace's attention count (C-1082): the number on the rail's Overview entry and in the Needs you heading.
  const counts = useWorkspaceCounts(wsId);
  const onboarding = useMeOnboarding();
  // (P5-POLISH-11) The same items as the Needs you panel below: app setup requests count too (owners and admins only).
  const orgData = useOrg(orgId);
  const isOrgAdmin = orgData.data?.my_role === "owner" || orgData.data?.my_role === "admin";
  const setups = useOrgSetupRequests(orgId, isOrgAdmin);
  const needs = counts.count + (isOrgAdmin ? (setups.data?.length ?? 0) : 0);
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
                  <p className="text-ink">{activityLine(e, byId.get(e.agent_id)?.name ?? "An agent", e.connection_id ? accountNames.get(e.connection_id) : undefined)}</p>
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
