"use client";

import { useParams } from "next/navigation";

import { AgentsList } from "@/components/agents-list";
import { ErrorState, Loading } from "@/components/states";
import { useAgents, useResolve } from "@/lib/api/queries";

import { CreateAgentDialog } from "./create-agent-dialog";

export function AgentsView() {
  const { org, ws } = useParams<{ org: string; ws: string }>();
  const resolved = useResolve(org, ws);
  const wsId = resolved.data?.workspace?.id;
  const agents = useAgents(wsId);

  if (resolved.isPending) return <Loading />;
  if (resolved.isError) return <ErrorState error={resolved.error} onRetry={() => resolved.refetch()} />;
  if (!wsId) return <ErrorState error={{ code: "not_found" }} />;
  return (
    <div className="max-w-4xl space-y-4">
      <div className="flex items-center justify-between">
        <h1 className="text-xl font-semibold">Agents</h1>
        <CreateAgentDialog wsId={wsId} />
      </div>
      {agents.isPending ? <Loading /> : null}
      {agents.isError ? <ErrorState error={agents.error} onRetry={() => agents.refetch()} /> : null}
      {agents.data ? <AgentsList agents={agents.data.items} base={`/o/${org}/w/${ws}/agents`} /> : null}
    </div>
  );
}
