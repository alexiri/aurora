"use client";

import { useParams } from "next/navigation";

import { ErrorState, Loading } from "@/components/states";
import { Fold } from "@/components/ui/fold";
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
    <div>
      <PageHeader
        title="Skills"
        description="A skill is a set of instructions that teaches an agent how to do one job your way, such as writing a release note or triaging an issue. Every agent in this workspace can use them."
      />
      <SkillsEditor scope={{ kind: "workspace", id: wsId }} readOnly={!canOwn} orgId={workspace.data.org_id} />
      <Fold summary="How skills combine" className="mt-6" data-testid="skills-combine">
        <p className="mt-2 text-ink-2">
          Skills come from your organization and from this workspace. If both have a skill with the same name, the workspace&apos;s version is used. New and
          changed skills reach your agents shortly.
        </p>
      </Fold>
    </div>
  );
}
