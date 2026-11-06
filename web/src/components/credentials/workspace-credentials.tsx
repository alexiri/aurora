"use client";

import { useParams } from "next/navigation";

import { ErrorState, Loading } from "@/components/states";
import { useResolve, useWorkspace } from "@/lib/api/queries";

import { CredentialsView } from "./credentials-view";

export function WorkspaceCredentials() {
  const { org, ws } = useParams<{ org: string; ws: string }>();
  const resolved = useResolve(org, ws);
  const wsId = resolved.data?.workspace?.id;
  const workspace = useWorkspace(wsId);
  if (resolved.isPending || (wsId && workspace.isPending)) return <Loading />;
  if (resolved.isError) return <ErrorState error={resolved.error} onRetry={() => resolved.refetch()} />;
  if (workspace.isError) return <ErrorState error={workspace.error} onRetry={() => workspace.refetch()} />;
  if (!wsId || !workspace.data) return <ErrorState error={{ code: "not_found" }} />;
  return (
    <CredentialsView
      title="Credentials"
      scope={{ kind: "workspace", id: wsId }}
      orgId={workspace.data.org_id}
      canOwn={workspace.data.my_role === "owner"}
    />
  );
}
