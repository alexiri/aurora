"use client";

import { useParams } from "next/navigation";

import { ApprovalsView } from "@/components/approvals/approvals-view";
import { ErrorState, Loading } from "@/components/states";
import { useResolve, useWorkspace } from "@/lib/api/queries";

export default function Page() {
  const { org, ws } = useParams<{ org: string; ws: string }>();
  const resolved = useResolve(org, ws);
  const workspace = useWorkspace(resolved.data?.workspace?.id);
  if (resolved.isPending) return <Loading />;
  if (resolved.isError) return <ErrorState error={resolved.error} onRetry={() => resolved.refetch()} />;
  if (!resolved.data.workspace) return <ErrorState error={{ code: "not_found" }} />;
  // Wait for the role before deciding, so owners and approvers never see a flash of "approver required".
  if (workspace.isPending) return <Loading />;
  if (workspace.isError) return <ErrorState error={workspace.error} onRetry={() => workspace.refetch()} />;
  // Phase 2, RFE-012 (D-55): the approvals page exists only for `can_approve` (owner or approver).
  const role = workspace.data.my_role;
  if (role !== "owner" && role !== "approver") {
    return <ErrorState error={{ code: "approver_required" }} />;
  }
  return <ApprovalsView wsId={resolved.data.workspace.id} />;
}
