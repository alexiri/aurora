"use client";

import { useParams } from "next/navigation";

import { ErrorState, Loading } from "@/components/states";
import { useAgents, useResolve, useWorkspace } from "@/lib/api/queries";

import { PolicyView } from "./policy-view";

export function WorkspacePolicy() {
  const { org, ws } = useParams<{ org: string; ws: string }>();
  const resolved = useResolve(org, ws);
  const wsId = resolved.data?.workspace?.id;
  const workspace = useWorkspace(wsId);
  const agents = useAgents(wsId);
  if (resolved.isPending || (wsId && (workspace.isPending || agents.isPending))) return <Loading />;
  if (resolved.isError) return <ErrorState error={resolved.error} onRetry={() => resolved.refetch()} />;
  if (workspace.isError) return <ErrorState error={workspace.error} onRetry={() => workspace.refetch()} />;
  if (!wsId || !workspace.data) return <ErrorState error={{ code: "not_found" }} />;
  return (
    <PolicyView
      title="Policy"
      scope={{ kind: "workspace", id: wsId }}
      orgId={workspace.data.org_id}
      canEdit={workspace.data.my_role === "owner"}
      agents={agents.data?.items ?? []}
    />
  );
}
