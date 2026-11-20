/**
 * (P5-REFINE-17 item 12, org-9; shared with P5-REFINE-16) What each role can do, in words. One table so the promotion
 * dialog, the "What can each role do?" fold and the invite dialog say the same thing. Nothing here is enforced: the
 * backend decides.
 */
export type OrgRoleName = "member" | "admin" | "owner";

export type WorkspaceRoleName = "member" | "approver" | "owner";
export interface RoleCan {
  label: string;
  summary: string;
  can: string[];
}

export const ROLE_CAN: { org: Record<OrgRoleName, RoleCan>; workspace: Record<WorkspaceRoleName, RoleCan> } = {
  // (P5-REFINE-16 workspace-5) The workspace's own roles. `can` of Owner lists only what an owner adds to an approver's.
  workspace: {
    member: { label: "Member", summary: "Chats with agents; cannot decide approvals.", can: ["Chat with the agents", "See what they can use"] },
    approver: { label: "Approver", summary: "Chats with agents and decides approvals.", can: ["Everything a member can do", "Decide approvals"] },
    owner: {
      label: "Owner",
      summary: "Manages the workspace, its access and settings, and decides approvals.",
      can: ["Add and remove people", "Change other people's roles", "Delete this workspace"],
    },
  },
  org: {
    member: {
      label: "Member",
      summary: "Chats with agents in the workspaces they belong to.",
      can: ["Use the workspaces they are added to", "Connect their own accounts", "Add their own AI key where the organization allows it"],
    },
    admin: {
      label: "Admin",
      summary: "Runs the organization day to day. Cannot delete it.",
      can: ["Invite and remove people", "Set up apps and chat platforms", "Change internet and AI model settings"],
    },
    owner: {
      label: "Owner",
      summary: "Everything an admin can do, and the things that cannot be undone.",
      can: ["Make and remove admins and owners", "Change sign-in and security settings", "Pause or delete the organization"],
    },
  },
};
