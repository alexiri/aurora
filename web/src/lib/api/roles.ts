import { useCanApprove } from "@/lib/api/approvals";
import { useMe, useOrg, useResolve, useWorkspace } from "@/lib/api/queries";
import type { AdminTopic, Level, OrgTopic, WorkspaceTopic } from "@/lib/routes";

/** Who is looking: what decides which topics a level lists (C-961, `14` "Who sees what"). */
export interface ViewerRoles {
  instanceAdmin: boolean;
  /** The viewer's role in the organization, when the level has one. */
  org?: "owner" | "admin" | "member";
  /** The viewer's role in the workspace, when the level has one. */
  workspace?: "owner" | "approver" | "member";
}

const ADMIN_TOPICS: AdminTopic[] = ["overview", "agents", "apps", "ai-models", "internet-access", "people", "chat-platforms", "data", "platform", "settings"];
const ORG_TOPICS: OrgTopic[] = ["workspaces", "agents", "apps", "ai-models", "internet-access", "people", "chat-platforms", "data", "settings"];
const ORG_MEMBER_TOPICS: OrgTopic[] = ["workspaces", "ai-models", "internet-access"];

export function visibleTopics(level: "admin", roles: ViewerRoles): AdminTopic[];
export function visibleTopics(level: "org", roles: ViewerRoles): OrgTopic[];
export function visibleTopics(level: "workspace", roles: ViewerRoles): WorkspaceTopic[];
export function visibleTopics(level: Level, roles: ViewerRoles): string[] {
  if (level === "admin") return roles.instanceAdmin ? ADMIN_TOPICS : [];
  if (level === "org") {
    // An instance admin acting in an organization sees its pages exactly as its owner would.
    const role = roles.org ?? (roles.instanceAdmin ? "owner" : undefined);
    if (role === "owner" || role === "admin") return ORG_TOPICS;
    return role === "member" ? ORG_MEMBER_TOPICS : [];
  }
  const role = roles.workspace ?? (roles.instanceAdmin ? "owner" : undefined);
  if (!role) return [];
  const canApprove = role === "owner" || role === "approver";
  return [
    "overview",
    "agents",
    "skills",
    "apps",
    "internet-access",
    ...(canApprove ? ["approvals"] : []),
    "activity",
    ...(role === "owner" ? ["people", "settings"] : []),
  ];
}

/** The viewer's roles for the page's level: the signed-in person, the organization and the workspace in the URL. */
export function useViewerRoles(orgSlug?: string, wsSlug?: string): ViewerRoles & { ready: boolean } {
  const me = useMe();
  const resolved = useResolve(orgSlug, wsSlug);
  const org = useOrg(orgSlug ? resolved.data?.org.id : undefined);
  const workspace = useWorkspace(wsSlug ? resolved.data?.workspace?.id : undefined);
  const canApprove = useCanApprove(wsSlug ? resolved.data?.workspace?.id : undefined);
  const instanceAdmin = me.data?.user.is_instance_admin === true;
  const resolvedDone = !orgSlug || !resolved.isPending;
  const ready =
    !me.isPending &&
    resolvedDone &&
    (!orgSlug || !resolved.data || !org.isPending) &&
    (!wsSlug || !resolved.data?.workspace || !workspace.isPending);
  return {
    instanceAdmin,
    org: org.data?.my_role,
    workspace: workspace.data?.my_role ?? (canApprove ? "approver" : undefined),
    ready,
  };
}
