/**
 * (P5-REFINE-17 item 12, org-9; shared with P5-REFINE-16) What each role can do, in words. One table so the promotion
 * dialog, the "What can each role do?" fold and the invite dialog say the same thing. Nothing here is enforced: the
 * backend decides.
 */
export type OrgRoleName = "member" | "admin" | "owner";

export const ROLE_CAN: Record<"org", Record<OrgRoleName, { label: string; summary: string; can: string[] }>> = {
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
