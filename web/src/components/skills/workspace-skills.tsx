"use client";

import { useParams } from "next/navigation";
import { useState } from "react";

import { ErrorState, Loading } from "@/components/states";
import { Fold } from "@/components/ui/fold";
import { PageHeader } from "@/components/ui/page-header";
import { useMe, useResolve, useWorkspace } from "@/lib/api/queries";
import { nameInSentence, workspaceDisplayName } from "@/lib/workspace-name";

import { AddSkillButton } from "./add-skill-button";
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
  const me = useMe();
  const [creating, setCreating] = useState(false);
  if (resolved.isPending || (wsId && workspace.isPending)) return <Loading />;
  if (resolved.isError) return <ErrorState error={resolved.error} onRetry={() => resolved.refetch()} />;
  if (workspace.isError) return <ErrorState error={workspace.error} onRetry={() => workspace.refetch()} />;
  if (!wsId || !workspace.data) return <ErrorState error={{ code: "not_found" }} />;
  const canOwn = workspace.data.my_role === "owner";
  return (
    <div>
      <PageHeader
        title="Skills"
        description={`Skills every agent in ${nameInSentence(workspaceDisplayName(workspace.data, me.data?.user))} can use`}
        actions={canOwn ? <AddSkillButton onClick={() => setCreating(true)} /> : null}
      />
      <SkillsEditor scope={{ kind: "workspace", id: wsId }} readOnly={!canOwn} orgId={workspace.data.org_id} create={{ open: creating, onOpenChange: setCreating }} />
      <Fold summary="How skills combine" className="mt-6" data-testid="skills-combine">
        <p className="mt-2 text-ink-2">
          Skills come from your organization and from this workspace. If both have a skill with the same name, the workspace&apos;s version is used. New and
          changed skills reach your agents shortly.
        </p>
      </Fold>
    </div>
  );
}
