"use client";

import { useParams } from "next/navigation";

import { ErrorState, Loading } from "@/components/states";
import { useAccess } from "@/lib/api/account";
import { useAgents, useResolve, useSession, useWorkspace } from "@/lib/api/queries";
import type { Agent } from "@/lib/api/queries";

import { PolicyView } from "./policy-view";

/** Workspace members with `can_use`: the people a `user` identity or a simulated speaker can name. */
function WorkspacePolicyBody({ wsId, orgId, agents }: { wsId: string; orgId: string; agents: Agent[] }) {
  const access = useAccess(wsId);
  const session = useSession();
  const users = (access.data?.items ?? []).map((b) => ({ id: b.user_id, label: `${b.display_name} (${b.email})` }));
  return (
    <PolicyView
      title="Policy"
      scope={{ kind: "workspace", id: wsId }}
      orgId={orgId}
      canEdit
      agents={agents}
      users={users}
      currentUserId={session.data?.user.id}
    />
  );
}

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
  // Workspace policy is `can_own` (I-27); members get the standard forbidden state.
  if (workspace.data.my_role !== "owner") return <ErrorState error={{ code: "forbidden" }} />;
  return <WorkspacePolicyBody wsId={wsId} orgId={workspace.data.org_id} agents={agents.data?.items ?? []} />;
}
