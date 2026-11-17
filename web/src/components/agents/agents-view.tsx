"use client";

import { useParams } from "next/navigation";

import { AgentsList } from "@/components/agents-list";
import { ErrorState, Loading } from "@/components/states";
import { MainAction } from "@/components/ui/main-action";
import { PageHeader } from "@/components/ui/page-header";
import { useAgents, useResolve, useWorkspace } from "@/lib/api/queries";
import { whoCanFromContacts } from "@/lib/who-can";

import { CreateAgentDialog } from "./create-agent-dialog";
import { WorkspaceByoRow } from "./workspace-byo";

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
        actions={
          workspace.data ? (
            <MainAction canAct={workspace.data.my_role === "owner"} whoCan={whoCanFromContacts("workspace_owner", workspace.data.contacts, "add_agent")}>
              <CreateAgentDialog wsId={wsId} />
            </MainAction>
          ) : null
        }
        className="mb-2"
      />
      {workspace.data ? <WorkspaceByoRow workspace={workspace.data} /> : null}
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
