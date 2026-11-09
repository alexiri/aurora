"use client";

import { useParams } from "next/navigation";

import { ErrorState, Loading } from "@/components/states";
import { Notice } from "@/components/ui/notice";
import { PageHeader } from "@/components/ui/page-header";
import { useResolve, useWorkspace } from "@/lib/api/queries";

import { SkillsEditor } from "./skills-view";

/**
 * Workspace-tier skills (RFE-020): every member who can use the workspace reads them; owners (and org
 * owners/admins) create, edit, enable/disable and delete. The API enforces it; members get the read-only view.
 */
export function WorkspaceSkills() {
  const { org, ws } = useParams<{ org: string; ws: string }>();
  const resolved = useResolve(org, ws);
  const wsId = resolved.data?.workspace?.id;
  const workspace = useWorkspace(wsId);
  if (resolved.isPending || (wsId && workspace.isPending)) return <Loading />;
  if (resolved.isError) return <ErrorState error={resolved.error} onRetry={() => resolved.refetch()} />;
  if (workspace.isError) return <ErrorState error={workspace.error} onRetry={() => workspace.refetch()} />;
  if (!wsId || !workspace.data) return <ErrorState error={{ code: "not_found" }} />;
  const canOwn = workspace.data.my_role === "owner";
  return (
    <div className="max-w-[1160px]">
      <PageHeader title="Workspace skills" description="Skills every agent in this workspace receives." />
      <div className="mb-4">
        <Notice role="status">
          An agent&apos;s own skill with the same name overrides a workspace skill, and a workspace skill overrides an organization skill. Workspace skills reach hosted
          agents on their next home sync.
        </Notice>
      </div>
      <SkillsEditor scope={{ kind: "workspace", id: wsId }} readOnly={!canOwn} />
    </div>
  );
}
