/**
 * How a workspace is named on screen (D-389). A personal workspace carries its owner's name in the database
 * ("Bob Builder"); each person reads it from their own side: "Your workspace" for theirs, "<First name>'s workspace"
 * for someone else's, the stored name for shared workspaces. The stored name is unchanged.
 */
export interface NamedWorkspace {
  name: string;
  /** The caller's own personal workspace (`/v1/me` memberships). */
  personal?: boolean;
  /** Set on a personal workspace: whose it is (`/v1/orgs/{org}/workspaces`). */
  personal_of?: { id: string; name: string } | null;
}

export function workspaceDisplayName(ws: NamedWorkspace, viewer: { id?: string } | null | undefined): string {
  if (ws.personal === true) return "Your workspace";
  const owner = ws.personal_of;
  if (!owner) return ws.name;
  if (viewer?.id !== undefined && owner.id === viewer.id) return "Your workspace";
  const parts = owner.name.trim().split(/\s+/).filter(Boolean);
  if (parts.length === 0) return ws.name;
  return `${parts[0]}'s workspace`;
}
