"use client";

import Link from "next/link";
import { useParams, useSearchParams } from "next/navigation";
import { type ReactNode } from "react";

import { AgentSummary } from "@/components/agents/agent-summary";
import { ErrorState, Loading } from "@/components/states";
import { Button, buttonVariants } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { PageHeader } from "@/components/ui/page-header";
import { useMeOnboarding, usePatchOnboarding } from "@/lib/api/onboarding";
import { useAgents, useOrg, useResolve, useSession, useWorkspace } from "@/lib/api/queries";
import type { GuideStepId } from "@/lib/onboarding/guide";
import { wsRoutes } from "@/lib/routes";
import { cn } from "@/lib/utils";

import { GettingStarted, OptionalNext } from "./getting-started";
import { startTour } from "./tour-events";

const STEPS: readonly GuideStepId[] = ["key", "access", "hello"];

function InfoCard({ id, title, children }: { id: string; title: string; children: ReactNode }) {
  return (
    <Card role="region" aria-labelledby={id}>
      <h2 id={id} className="font-display text-sm font-semibold text-ink">
        {title}
      </h2>
      <div className="mt-1.5 space-y-1 text-sm text-ink-2">{children}</div>
    </Card>
  );
}

/** The one sentence about the viewer's role that a person who is not an owner reads (C-927). */
export function roleSentence(role: string, agent: string): string | null {
  if (role === "owner") return null;
  return role === "approver"
    ? `You are an approver here: you can chat with ${agent} and answer what it asks you in Approvals. An owner sets up its accounts and access.`
    : `You are a member here: you can chat with ${agent} and see what it did. An owner sets up its accounts and access.`;
}

/**
 * The Welcome page (`14 §9`, C-927): "Welcome to <workspace>" (or "Welcome, <name>" in your own personal workspace), three
 * cards from the five sentences of `14 §Information architecture`, "Get <Agent> talking" and what to do next. It is where
 * a person lands until the guide is finished or skipped (the guide marks Welcome seen when it completes).
 */
export function WelcomeView() {
  const { org, ws } = useParams<{ org: string; ws: string }>();
  const search = useSearchParams();
  const resolved = useResolve(org, ws);
  const orgQ = useOrg(resolved.data?.org.id);
  const resolvedWs = resolved.data?.workspace;
  const wsFull = useWorkspace(resolvedWs?.id);
  const agents = useAgents(resolvedWs?.id);
  const session = useSession();
  const patch = usePatchOnboarding();
  useMeOnboarding();

  if (resolved.isPending) return <Loading />;
  if (resolved.isError) return <ErrorState error={resolved.error} onRetry={() => resolved.refetch()} />;
  if (!resolvedWs) return <ErrorState error={{ code: "not_found" }} />;
  if (wsFull.isError) return <ErrorState error={wsFull.error} onRetry={() => wsFull.refetch()} />;
  if (!wsFull.data) return <Loading />;
  const workspace = wsFull.data;

  const w = wsRoutes(org, ws);
  const isOwner = workspace.my_role === "owner";
  const orgName = orgQ.data?.name ?? resolved.data.org.name;
  const agent = agents.data?.items.find((a) => a.is_default) ?? agents.data?.items[0];
  const personal = session.data?.memberships.some((m) => m.workspaces.some((x) => x.id === workspace.id && x.personal)) === true;
  const stepParam = search.get("step");
  // `step=model` links (a chat's "no model" row) land on the AI key step, which now holds the model picker.
  const focus = STEPS.find((s) => s === (stepParam === "model" ? "key" : stepParam)) ?? null;
  const agentName = agent?.name ?? "Your agent";
  const role = roleSentence(workspace.my_role, agentName);

  return (
    <div className="flex flex-col gap-6">
      <PageHeader
        title={personal ? `Welcome, ${workspace.name}` : `Welcome to ${workspace.name}`}
        description={role ?? "A few minutes from here to your agent's first answer."}
        className="mb-0"
      />

      <div className="grid gap-3 md:grid-cols-3">
        <InfoCard id="welcome-where" title="Where you are">
          <p>
            This is <strong className="text-ink">{workspace.name}</strong>, in <strong className="text-ink">{orgName}</strong>.
            {agent ? ` ${agent.name} works here, on its own computer.` : " Agents work here, each on its own computer."}
          </p>
        </InfoCard>
        <InfoCard id="welcome-safe" title="How it stays safe">
          <p>
            Everything {agent ? agent.name : "an agent"} sends goes through Aurora&apos;s gateway. It holds the keys, applies the access you choose, and
            puts anything that needs your say in{" "}
            <Link href={w.approvals()} className="font-medium text-link underline">
              Approvals
            </Link>
            .
          </p>
        </InfoCard>
        <InfoCard id="welcome-agent" title={agent ? `${agent.name} is your agent` : "Your agent"}>
          {agent ? (
            <AgentSummary wsId={workspace.id} orgId={workspace.org_id} agent={agent} canOwn={isOwner} className="space-y-1" />
          ) : (
            <p>This workspace has no agent yet.</p>
          )}
        </InfoCard>
      </div>

      <GettingStarted org={org} ws={ws} wsId={workspace.id} orgId={workspace.org_id} isOwner={isOwner} variant="welcome" focus={focus} />

      <OptionalNext org={org} ws={ws} handle={agent?.handle} agentName={agent?.name} isOwner={isOwner} />

      <div className="flex flex-wrap items-center gap-3 border-t border-line pt-4">
        <Button
          variant="outline"
          onClick={() => {
            patch.mutate({ welcome_seen: true });
            startTour();
          }}
        >
          Show me around
        </Button>
        <Link href={w.home()} className={cn(buttonVariants({ variant: "ghost" }))} onClick={() => patch.mutate({ welcome_seen: true })}>
          Skip for now
        </Link>
        <span className="text-sm text-muted">The same guide stays on your Overview until it is done.</span>
      </div>
    </div>
  );
}
