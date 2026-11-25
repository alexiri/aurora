// (FIX-235, D-479, C-1162) The chat's visibility line: who can use the agent and who can read
// the conversation. One pure function, `visibilityLines`, so the thread header, the empty state
// and (FIX-236) the emergency-access line all draw the same text. The result is a list of lines —
// one per row of D-479's table today; FIX-236 appends the emergency-access line — and each line
// is a plain string, so React renders it as text, never HTML, and names with `<` or `&` stay
// literal.
//
// The count N and the owner names come from `Workspace.audience`: the workspace's people in
// their own right (D-477) and its own-right owners (at most 5, the caller first if an owner).
// An organization role puts nobody in either — an administrator who is not one of the
// workspace's people can neither use the agent nor read its chats.

import type { components } from "@/lib/api/schema";
import { emergencyVisibilityLine } from "@/lib/emergency";
import { personName } from "@/lib/workspace-name";

type S = components["schemas"];
export type WorkspaceAudience = S["WorkspaceAudience"];

/** What one line needs: the conversation, the reader and the workspace's audience. */
export interface VisibilityInput {
  /** The conversation's kind (`direct`, `group`, …). */
  kind: string;
  /** The reader's own direct conversation. */
  ownDirect: boolean;
  /** Display name of the person whose direct conversation an owner reads (read-only view). */
  ownerView: string | null;
  /** `Workspace.audience` (C-1162); `null` when the caller holds no own access. */
  audience: WorkspaceAudience | null;
  /** The reader's user id; the owner names never include them. */
  readerId: string | null;
  /** The reader's own `WorkspaceRole` (`null` = no own access). */
  myRole: string | null;
}

/**
 * At most three names, then "and K others" (K counted from `owners_total`, not from the list —
 * the API caps `owners` at five). `others` excludes the reader already.
 */
function nameList(others: { display_name: string }[], totalOthers: number): string {
  const shown = others.slice(0, 3).map((o) => personName(o.display_name));
  if (totalOthers > shown.length) {
    return `${shown.join(", ")} and ${totalOthers - shown.length} others`;
  }
  return shown.join(", ");
}

export function visibilityLines(input: VisibilityInput): string[] {
  const { kind, ownDirect, ownerView, audience, readerId, myRole } = input;

  // (FIX-236, C-1165) While an emergency grant is active, its line is appended whatever else the
  // table says — the emergency reader holds no own access, so `audience` is the only place the
  // grant shows. The workspace's own line stays unchanged; the emergency line is the last one.
  const emergency = audience?.emergency;
  const emergencyLine = emergency?.active ? emergencyVisibilityLine(emergency) : null;

  // An owner reading another person's direct chat: the read-only view's line, unchanged.
  if (ownerView) {
    const lines = [`Only ${ownerView} and the workspace's owners can see this conversation.`];
    return emergencyLine ? [...lines, emergencyLine] : lines;
  }

  const n = audience?.people ?? 0;
  if (n < 2) {
    const lines = ["Only you can use this agent and read its conversations."];
    return emergencyLine ? [...lines, emergencyLine] : lines;
  }

  const use = `All ${n} people in this workspace can use this agent`;
  // The owners besides the reader: the audience names the reader first when they are one.
  const others = (audience?.owners ?? []).filter((o) => o.id !== readerId);
  const totalOthers = Math.max(0, (audience?.owners_total ?? 0) - (myRole === "owner" ? 1 : 0));

  if (kind === "direct" && ownDirect) {
    if (totalOthers < 1) {
      // The reader the only owner, or nobody holds the workspace's ownership.
      const lines = [`${use}. Only you can read this chat.`];
      return emergencyLine ? [...lines, emergencyLine] : lines;
    }
    if (myRole === "owner") {
      const who = totalOthers === 1 ? "the other owner" : "the other owners";
      const lines = [`${use}. This chat is visible to you and ${who}: ${nameList(others, totalOthers)}.`];
      return emergencyLine ? [...lines, emergencyLine] : lines;
    }
    if (totalOthers === 1) {
      const lines = [`${use}. This chat is visible to you and the workspace's owner, ${personName(others[0]?.display_name) ?? "the owner"}.`];
      return emergencyLine ? [...lines, emergencyLine] : lines;
    }
    const lines = [`${use}. This chat is visible to you and the workspace's owners: ${nameList(others, totalOthers)}.`];
    return emergencyLine ? [...lines, emergencyLine] : lines;
  }

  // A conversation that is not the reader's direct chat: everyone with access reads it.
  const lines = [`${use} and read this conversation.`];
  return emergencyLine ? [...lines, emergencyLine] : lines;
}
