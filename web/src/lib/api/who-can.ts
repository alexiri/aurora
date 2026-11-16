"use client";

import { useOrg, useWorkspace } from "@/lib/api/queries";
import { whoCanFromContacts, type WhoCanRole, type WhoCanValue } from "@/lib/who-can";

/**
 * (C-981) Who to ask for something the viewer cannot do in this workspace or organization: the role's people from the
 * place's `contacts` (the query the page has already loaded). Until it loads the answer names only the role.
 * Pass `workspaceId` for a workspace role, `orgId` for an organization one.
 */
export function useWhoCan(place: { workspaceId?: string; orgId?: string }, role: WhoCanRole, action: string): WhoCanValue {
  const workspace = useWorkspace(place.workspaceId);
  const org = useOrg(place.workspaceId ? undefined : place.orgId);
  return whoCanFromContacts(role, place.workspaceId ? workspace.data?.contacts : org.data?.contacts, action);
}
