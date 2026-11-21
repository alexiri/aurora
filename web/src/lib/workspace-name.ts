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
  const first = firstNameOf(owner.name);
  // A person with no display name is stored by their address: never an e-mail as a name ("dave@example.test's workspace").
  if (first === null) return "Workspace of a person without a name";
  return `${first}'s workspace`;
}

/** A workspace's name after a verb ("Back to your workspace", "Go to your workspace"): only "Your workspace" changes its capital. */
export function nameInSentence(name: string): string {
  return name === "Your workspace" ? "your workspace" : name;
}

/** "Bob Builder" → "Bob"; an address → the capitalised local part ("dave@example.test" → "Dave"); nothing usable → null. */
export function firstNameOf(name: string | null | undefined): string | null {
  const text = (name ?? "").trim();
  if (!text) return null;
  if (text.includes("@")) {
    const local = (text.split("@")[0] ?? "").trim();
    return local ? `${local.charAt(0).toUpperCase()}${local.slice(1)}` : null;
  }
  return text.split(/\s+/)[0] ?? null;
}
