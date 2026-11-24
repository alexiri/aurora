import { WorkspaceManage } from "@/components/workspace/workspace-manage";

/** (FIX-235) The manage page for a workspace an administrator is not in (`/o/<org>/workspaces/<ws>`). */
export default function WorkspaceManagePage() {
  return <WorkspaceManage />;
}
