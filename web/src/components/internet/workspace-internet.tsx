"use client";

import { InternetLevel } from "@/components/internet/internet-level";
import { ErrorState, Loading } from "@/components/states";
import { AltitudeScope, MoreControlsLine } from "@/components/ui/altitude";
import { useAccess } from "@/lib/api/account";
import { useEffectiveGovernance } from "@/lib/api/governance";
import { useAgents, useOrg, useSession, type Workspace } from "@/lib/api/queries";
import { whoCanFromContacts } from "@/lib/who-can";
import { personName } from "@/lib/workspace-name";

/** (P5-IA-2, C-965) A workspace's Internet access: owners edit it; members read the limits and see who to ask. */
export function WorkspaceInternet({ workspace: w, orgSlug, wsSlug }: { workspace: Workspace; orgSlug: string; wsSlug: string }) {
  const canOwn = w.my_role === "owner";
  const eff = useEffectiveGovernance(w.org_id);
  const org = useOrg(w.org_id);
  const agents = useAgents(canOwn ? w.id : undefined);
  const access = useAccess(w.id, canOwn);
  const session = useSession();
  if (eff.isPending || (canOwn && agents.isPending)) return <Loading />;
  if (eff.isError) return <ErrorState error={eff.error} onRetry={() => eff.refetch()} />;
  const users = (access.data?.items ?? []).map((b) => ({ id: b.user_id, label: `${personName(b.display_name)} (${b.email})` }));
  return (
    <AltitudeScope>
    <InternetLevel
      level="workspace"
      scope={{ kind: "workspace", id: w.id }}
      orgId={w.org_id}
      orgSlug={orgSlug}
      wsSlug={wsSlug}
      canEdit={canOwn}
      eff={eff.data}
      whoCanHere={whoCanFromContacts("workspace_owner", w.contacts, "change_this")}
      whoCanOrg={whoCanFromContacts("org_admin", org.data?.contacts, "change_this")}
      agents={agents.data?.items ?? []}
      users={users}
      currentUserId={session.data?.user.id}
    />
    <MoreControlsLine className="pt-4" />
    </AltitudeScope>
  );
}
