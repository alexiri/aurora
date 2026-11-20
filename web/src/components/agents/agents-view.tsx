"use client";

import { useParams } from "next/navigation";

import { AgentsList } from "@/components/agents-list";
import { ErrorState, Loading } from "@/components/states";
import { Altitude, AltitudeScope, MoreControlsLine } from "@/components/ui/altitude";
import { MainAction } from "@/components/ui/main-action";
import { PageHeader } from "@/components/ui/page-header";
import { useAgentNeedsCounts } from "@/lib/api/agent-needs";
import { useApprovalsStream } from "@/lib/api/approvals";
import { useAgents, useResolve, useWorkspace } from "@/lib/api/queries";
import { whoCanFromContacts } from "@/lib/who-can";

import { CreateAgentDialog } from "./create-agent-dialog";
import { WorkspaceByoRow } from "./workspace-byo";

/** Keeps the pending approvals live (the stream refetches them), so a row's count drops when someone decides. */
function ApprovalsStreamOpen({ wsId }: { wsId: string }) {
  useApprovalsStream(wsId);
  return null;
}

export function AgentsView() {
  const { org, ws } = useParams<{ org: string; ws: string }>();
  const resolved = useResolve(org, ws);
  const wsId = resolved.data?.workspace?.id;
  const agents = useAgents(wsId);
  const workspace = useWorkspace(wsId);
  // (P5-REFINE-11 item 11) Approvals waiting for the viewer, per agent row; nothing for a person who cannot decide.
  const needs = useAgentNeedsCounts(wsId);

  if (resolved.isPending) return <Loading />;
  if (resolved.isError) return <ErrorState error={resolved.error} onRetry={() => resolved.refetch()} />;
  if (!wsId) return <ErrorState error={{ code: "not_found" }} />;
  return (
    <AltitudeScope>
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
      {workspace.data ? (
        <Altitude id="agent.byo" level="setup">
          <WorkspaceByoRow workspace={workspace.data} />
        </Altitude>
      ) : null}
      {agents.isPending ? <Loading /> : null}
      {agents.isError ? <ErrorState error={agents.error} onRetry={() => agents.refetch()} /> : null}
      {agents.data ? (
        <AgentsList
          agents={agents.data.items}
          base={`/o/${org}/w/${ws}/agents`}
          showTokenBadges={workspace.data?.my_role === "owner"}
          needs={needs.counts}
        />
      ) : null}
      {needs.canDecide && wsId ? (
        <ApprovalsStreamOpen wsId={wsId} />
      ) : null}
      <MoreControlsLine />
    </div>
    </AltitudeScope>
  );
}
