"use client";

import Link from "next/link";
import { useParams, useSearchParams } from "next/navigation";
import { useEffect, type ReactNode } from "react";

import { ErrorState, Loading } from "@/components/states";
import { Button, buttonVariants } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { PageHeader } from "@/components/ui/page-header";
import { useMeOnboarding, usePatchOnboarding } from "@/lib/api/onboarding";
import { useAgents, useOrg, useResolve, useWorkspace } from "@/lib/api/queries";
import type { GuideStepId } from "@/lib/onboarding/guide";
import { cn } from "@/lib/utils";

import { GettingStarted, OptionalNext } from "./getting-started";
import { startTour } from "./tour-events";

const STEPS: readonly GuideStepId[] = ["key", "access", "model", "hello"];

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

/** The Welcome page (`14 §Onboarding` 2): where you are, then "Get Daneel talking". */
export function WelcomeView() {
  const { org, ws } = useParams<{ org: string; ws: string }>();
  const search = useSearchParams();
  const resolved = useResolve(org, ws);
  const orgQ = useOrg(resolved.data?.org.id);
  const resolvedWs = resolved.data?.workspace;
  const wsFull = useWorkspace(resolvedWs?.id);
  const agents = useAgents(resolvedWs?.id);
  const onboarding = useMeOnboarding();
  const patch = usePatchOnboarding();
  const seen = onboarding.data?.welcome_seen;

  // Opening the page is what "seen" means: the next sign-in lands on Overview.
  useEffect(() => {
    if (seen === false && !patch.isPending) patch.mutate({ welcome_seen: true });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [seen]);

  if (resolved.isPending) return <Loading />;
  if (resolved.isError) return <ErrorState error={resolved.error} onRetry={() => resolved.refetch()} />;
  if (!resolvedWs) return <ErrorState error={{ code: "not_found" }} />;
  if (wsFull.isError) return <ErrorState error={wsFull.error} onRetry={() => wsFull.refetch()} />;
  if (!wsFull.data) return <Loading />;
  const workspace = wsFull.data;

  const base = `/o/${org}/w/${ws}`;
  const isOwner = workspace.my_role === "owner";
  const orgName = orgQ.data?.name ?? resolved.data.org.name;
  const agent = agents.data?.items.find((a) => a.is_default) ?? agents.data?.items[0];
  const stepParam = search.get("step");
  const focus = STEPS.find((s) => s === stepParam) ?? null;
  const role =
    workspace.my_role === "owner"
      ? { title: "You are the owner", text: "You can add keys and credentials, set the rules, approve what the agent asks for, and invite people." }
      : workspace.my_role === "approver"
        ? { title: "You are an approver", text: "You can chat with the agent and decide what it asks for. An owner manages keys and rules." }
        : { title: "You are a member", text: "You can chat with the agent and see its activity. An owner manages keys and rules." };

  return (
    <div className="flex flex-col gap-6">
      <PageHeader title={`Welcome to ${workspace.name}`} description="A few minutes from here to your agent's first answer." className="mb-0" />

      <div className="grid gap-3 md:grid-cols-3">
        <InfoCard id="welcome-where" title="Where you are">
          <p>
            This is <strong className="text-ink">{workspace.name}</strong>, your workspace in <strong className="text-ink">{orgName}</strong>.
          </p>
          <p>Workspaces hold agents, credentials and rules.</p>
        </InfoCard>
        <InfoCard id="welcome-agent" title={agent ? `${agent.name} is your agent` : "Your agent"}>
          <p>
            {agent ? agent.name : "An agent"} works on its own computer and reaches the internet only through Aurora&apos;s gateway. The gateway adds
            credentials and asks you before risky actions.
          </p>
        </InfoCard>
        <InfoCard id="welcome-role" title={role.title}>
          <p>{role.text}</p>
        </InfoCard>
      </div>

      <GettingStarted org={org} ws={ws} wsId={workspace.id} orgId={workspace.org_id} isOwner={isOwner} variant="welcome" focus={focus} />

      <OptionalNext org={org} ws={ws} handle={agent?.handle} isOwner={isOwner} />

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
        <Link href={base} className={cn(buttonVariants({ variant: "ghost" }))} onClick={() => patch.mutate({ welcome_seen: true })}>
          Skip for now
        </Link>
        <span className="text-sm text-muted">The same guide stays on your Overview until it is done.</span>
      </div>
    </div>
  );
}
