"use client";

import { useParams } from "next/navigation";

import { AgentsList } from "@/components/agents-list";
import { ErrorState, Loading } from "@/components/states";
import { useAgents, useResolve } from "@/lib/api/queries";

export default function WorkspaceOverviewPage() {
  const { org, ws } = useParams<{ org: string; ws: string }>();
  const resolved = useResolve(org, ws);
  const wsId = resolved.data?.workspace?.id;
  const agents = useAgents(wsId);

  if (resolved.isPending) return <Loading />;
  if (resolved.isError) return <ErrorState error={resolved.error} onRetry={() => resolved.refetch()} />;
  if (!resolved.data.workspace) {
    return <ErrorState error={{ code: "not_found" }} />;
  }

  return (
    <div className="space-y-6">
      <h1 className="text-xl font-semibold">{resolved.data.workspace.name}</h1>
      <section aria-labelledby="agents-h" className="space-y-2">
        <h2 id="agents-h" className="text-sm font-medium text-neutral-600">
          Agents
        </h2>
        {agents.isPending ? <Loading /> : null}
        {agents.isError ? <ErrorState error={agents.error} onRetry={() => agents.refetch()} /> : null}
        {agents.data ? <AgentsList agents={agents.data.items} /> : null}
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
