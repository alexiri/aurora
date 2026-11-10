"use client";

import Link from "next/link";
import { useParams } from "next/navigation";

import { Avatar } from "@/components/agent-avatar";
import { CreateAgentDialog } from "@/components/agents/create-agent-dialog";
import { decisionTone } from "@/components/approvals/activity-view";
import { ErrorState, Loading } from "@/components/states";
import { Badge } from "@/components/ui/badge";
import { buttonVariants } from "@/components/ui/button";
import { GettingStarted } from "@/components/onboarding/getting-started";
import { PageHeader } from "@/components/ui/page-header";
import { Section } from "@/components/ui/section";
import { StatusDot } from "@/components/ui/status-dot";
import { countdown, useActivity, useApprovalsStream, useCanApprove, useNow, usePendingApprovals } from "@/lib/api/approvals";
import { useAgents, useResolve, useWorkspace } from "@/lib/api/queries";
import { cn } from "@/lib/utils";

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
  const pending = usePendingApprovals(wsId, canApprove);
  const activity = useActivity(wsId, { agent: "", decision: "", host: "" });
  const now = useNow(30_000);
  const clock = useNow(1000);
  const base = `/o/${org}/w/${ws}`;
  const urgent = pending.data?.items.slice(0, 2) ?? [];
  const count = pending.data?.items.length ?? 0;
  const recent = (activity.data?.pages[0]?.items ?? []).slice(0, 6);
  const byId = new Map(agents.data?.items.map((a) => [a.id, a]) ?? []);

  return (
    <div className="flex flex-col gap-8">
      <PageHeader title={name} description="Your agents, what they asked for and what they did." className="mb-0" />

      {canApprove && count > 0 ? (
        <section
          aria-labelledby="approvals-h"
          className="rounded-card border border-pending-line bg-pending-panel p-4 text-ink sm:p-5"
        >
          <div className="flex flex-wrap items-center justify-between gap-2">
            <h2 id="approvals-h" className="font-display text-base font-semibold text-pending-ink">
              Approvals
              <span className="sr-only"> waiting</span>{" "}
              <Badge tone="pending" aria-label={`${count} pending`}>
                {count}
              </Badge>
            </h2>
            <Link
              href={`${base}/approvals`}
              className={cn(buttonVariants({ variant: "outline", size: "sm" }))}
            >
              Review all
            </Link>
          </div>
          <ul className="mt-3 space-y-2">
            {urgent.map((a) => (
              <li key={a.id} className="flex flex-wrap items-center gap-x-3 gap-y-1 rounded-control bg-surface px-3 py-2 text-sm">
                <span className="min-w-0 flex-1 truncate font-medium">{a.summary.title}</span>
                <span className="font-mono text-xs text-muted">{a.agent ? `@${a.agent.handle}` : "Unknown agent"}</span>
                <span className="text-xs font-medium tabular-nums text-pending-ink">{countdown(a.deadline, clock)}</span>
              </li>
            ))}
          </ul>
        </section>
      ) : null}

      <Section id="agents" title="Agents">
        {agents.isPending ? <Loading /> : null}
        {agents.isError ? <ErrorState error={agents.error} onRetry={() => agents.refetch()} /> : null}
        {agents.data ? (
          <ul aria-label="Agents" className="grid gap-3 sm:grid-cols-2 lg:grid-cols-3">
            {agents.data.items.map((a) => (
              <li key={a.id}>
                <Link
                  href={`${base}/agents/${a.handle}`}
                  className="flex h-full min-w-0 items-start gap-3 rounded-card border border-line bg-surface p-4 shadow-card hover:border-line-strong"
                >
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
              </li>
            ))}
            <li className="flex min-h-[5.5rem] items-center justify-center rounded-card border border-dashed border-line-strong bg-surface p-4">
              <CreateAgentDialog wsId={wsId} />
            </li>
          </ul>
        ) : null}
      </Section>

      <div className="grid gap-8 lg:grid-cols-2">
        <Section id="first-run" title="Getting started">
          {workspace.data ? (
            <GettingStarted org={org} ws={ws} wsId={wsId} orgId={orgId} isOwner={workspace.data.my_role === "owner"} variant="overview" />
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
