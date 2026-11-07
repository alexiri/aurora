"use client";

import Link from "next/link";
import { useParams } from "next/navigation";

import { AgentsList } from "@/components/agents-list";
import { ErrorState, Loading } from "@/components/states";
import { useCanApprove, usePendingApprovals } from "@/lib/api/approvals";
import { useAgents, useResolve } from "@/lib/api/queries";

export default function WorkspaceOverviewPage() {
  const { org, ws } = useParams<{ org: string; ws: string }>();
  const resolved = useResolve(org, ws);
  const wsId = resolved.data?.workspace?.id;
  const agents = useAgents(wsId);
  const canApprove = useCanApprove(wsId);
  const pending = usePendingApprovals(wsId ?? "", canApprove);

  if (resolved.isPending) return <Loading />;
  if (resolved.isError) return <ErrorState error={resolved.error} onRetry={() => resolved.refetch()} />;
  if (!resolved.data.workspace) {
    return <ErrorState error={{ code: "not_found" }} />;
  }

  return (
    <div className="space-y-6">
      <h1 className="text-xl font-semibold">{resolved.data.workspace.name}</h1>
      {canApprove ? (
      <section aria-labelledby="approvals-h" className="space-y-2">
        <h2 id="approvals-h" className="text-sm font-medium text-neutral-600">
          Approvals
        </h2>
        <Link href={`/o/${org}/w/${ws}/approvals`} className="text-sm underline">
          Pending approvals
          {pending.data ? (
            <span className="ml-2 rounded-full bg-amber-100 px-2 py-0.5 text-xs text-amber-800" aria-label={`${pending.data.items.length} pending`}>
              {pending.data.items.length}
            </span>
          ) : null}
        </Link>
      </section>
      ) : null}
      <section aria-labelledby="agents-h" className="space-y-2">
        <h2 id="agents-h" className="text-sm font-medium text-neutral-600">
          Agents
        </h2>
        {agents.isPending ? <Loading /> : null}
        {agents.isError ? <ErrorState error={agents.error} onRetry={() => agents.refetch()} /> : null}
        {agents.data ? <AgentsList agents={agents.data.items} base={`/o/${org}/w/${ws}/agents`} /> : null}
      </section>
      <section aria-labelledby="first-run-h" className="space-y-2">
        <h2 id="first-run-h" className="text-sm font-medium text-neutral-600">
          Getting started
        </h2>
        <p className="text-sm">Store an LLM key → grant it to Daneel → say hello.</p>
      </section>
    </div>
  );
}
