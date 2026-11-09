"use client";

import { useParams } from "next/navigation";

import { AgentsList } from "@/components/agents-list";
import { ErrorState, Loading } from "@/components/states";
import { PageHeader } from "@/components/ui/page-header";
import { useAgents, useResolve, useWorkspace } from "@/lib/api/queries";

import { CreateAgentDialog } from "./create-agent-dialog";

export function AgentsView() {
  const { org, ws } = useParams<{ org: string; ws: string }>();
  const resolved = useResolve(org, ws);
  const wsId = resolved.data?.workspace?.id;
  const agents = useAgents(wsId);
  const workspace = useWorkspace(wsId);

  if (resolved.isPending) return <Loading />;
  if (resolved.isError) return <ErrorState error={resolved.error} onRetry={() => resolved.refetch()} />;
  if (!wsId) return <ErrorState error={{ code: "not_found" }} />;
  return (
    <div className="space-y-4">
      <PageHeader
        title="Agents"
        description="The agents in this workspace."
        actions={<CreateAgentDialog wsId={wsId} />}
        className="mb-2"
      />
      {agents.isPending ? <Loading /> : null}
      {agents.isError ? <ErrorState error={agents.error} onRetry={() => agents.refetch()} /> : null}
      {agents.data ? (
        <AgentsList
          agents={agents.data.items}
          base={`/o/${org}/w/${ws}/agents`}
          showTokenBadges={workspace.data?.my_role === "owner"}
        />
      ) : null}
    </div>
  );
}
