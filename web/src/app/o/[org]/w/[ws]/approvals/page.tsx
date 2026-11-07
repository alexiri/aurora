"use client";

import { useParams } from "next/navigation";

import { ApprovalsView } from "@/components/approvals/approvals-view";
import { ErrorState, Loading } from "@/components/states";
import { useCanApprove } from "@/lib/api/approvals";
import { useResolve } from "@/lib/api/queries";

export default function Page() {
  const { org, ws } = useParams<{ org: string; ws: string }>();
  const resolved = useResolve(org, ws);
  const canApprove = useCanApprove(resolved.data?.workspace?.id);
  if (resolved.isPending) return <Loading />;
  if (resolved.isError) return <ErrorState error={resolved.error} onRetry={() => resolved.refetch()} />;
  if (!resolved.data.workspace) return <ErrorState error={{ code: "not_found" }} />;
  if (!canApprove) {
    return <ErrorState error={{ code: "approver_required" }} />;
  }
  return <ApprovalsView wsId={resolved.data.workspace.id} />;
}
