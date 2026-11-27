"use client";

import { useParams } from "next/navigation";

import { ErrorState, Loading } from "@/components/states";
import { ScreenChromeContext } from "@/components/ui/list-detail";
import { PageHeader } from "@/components/ui/page-header";
import { useMe, useResolve, useWorkspace } from "@/lib/api/queries";
import { isEmergencyReader } from "@/lib/emergency";
import { nameInSentence, workspaceDisplayName } from "@/lib/workspace-name";

import { useHeaderSlot } from "./header-slot";
import { SkillsList } from "./skills-list";

/**
 * Workspace-tier skills (RFE-020): every member who can use the workspace reads them; owners (and org
 * owners/admins) create, edit, switch off and remove them. The API enforces it; members get the read-only view.
 */
export function WorkspaceSkills() {
  const { org, ws } = useParams<{ org: string; ws: string }>();
  const resolved = useResolve(org, ws);
  const wsId = resolved.data?.workspace?.id;
  const workspace = useWorkspace(wsId);
  const me = useMe();
  const header = useHeaderSlot();
  if (resolved.isPending || (wsId && workspace.isPending)) return <Loading />;
  if (resolved.isError) return <ErrorState error={resolved.error} onRetry={() => resolved.refetch()} />;
  if (workspace.isError) return <ErrorState error={workspace.error} onRetry={() => workspace.refetch()} />;
  if (!wsId || !workspace.data) return <ErrorState error={{ code: "not_found" }} />;
  const name = nameInSentence(workspaceDisplayName(workspace.data, me.data?.user));
  return (
    <div>
      <PageHeader title="Skills" description={`Skills every agent in ${name} can use`} actions={header.slot} className={header.subScreen ? "hidden" : undefined} />
      <ScreenChromeContext.Provider value={header.chrome}>
        <SkillsList
          page={{
            kind: "workspace",
            wsId,
            orgId: workspace.data.org_id,
            label: name,
            canOwn: workspace.data.my_role === "owner",
            emergencyReader: isEmergencyReader(workspace.data),
          }}
        />
      </ScreenChromeContext.Provider>
    </div>
  );
}
