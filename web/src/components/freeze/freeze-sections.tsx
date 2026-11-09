"use client";

import {
  useAdminFreezeOrg,
  useFreezeAgent,
  useFreezeAuthority,
  useFreezeOrg,
  useFreezeWorkspace,
  type Frozen,
} from "@/lib/api/freeze";

import { DangerZone } from "@/components/ui/section";

import { FreezeControl, FrozenBanner } from "./freeze-panel";

/** Agent header: banner when frozen (for everyone), Freeze / Unfreeze for `can_own`. */
export function AgentFreeze({
  agentId,
  name,
  orgId,
  frozen,
  canOwn,
  inMenu = false,
}: {
  agentId: string;
  name: string;
  orgId: string;
  frozen: Frozen | null | undefined;
  canOwn: boolean;
  /** Compact header: Freeze is an overflow-menu entry (the banner and Unfreeze are unchanged). */
  inMenu?: boolean;
}) {
  const m = useFreezeAgent(agentId);
  const rank = useFreezeAuthority(orgId);
  // A workspace or organization freeze is shown by the shell banner (frozen-entity-banner).
  if (frozen && frozen.level !== "agent") return null;
  if (!canOwn) return frozen ? <FrozenBanner frozen={frozen} subject="agent" /> : null;
  return <FreezeControl subject="agent" name={name} frozen={frozen} callerRank={rank} freeze={m.freeze} unfreeze={m.unfreeze} inMenu={inMenu} />;
}

/** Workspace settings, danger zone (`can_own`). */
export function WorkspaceFreeze({
  wsId,
  name,
  orgId,
  frozen,
}: {
  wsId: string;
  name: string;
  orgId: string;
  frozen: Frozen | null | undefined;
}) {
  const m = useFreezeWorkspace(wsId);
  const rank = useFreezeAuthority(orgId);
  return (
    <DangerZone
      id="freeze-workspace"
      title="Freeze this workspace"
      description="Stops every agent in the workspace until you unfreeze it. Nothing is deleted."
    >
      <FreezeControl subject="workspace" name={name} frozen={frozen} callerRank={rank} freeze={m.freeze} unfreeze={m.unfreeze} />
    </DangerZone>
  );
}

/** Organization settings, danger zone (org owners). */
export function OrgFreeze({ orgId, name, frozen }: { orgId: string; name: string; frozen: Frozen | null | undefined }) {
  const m = useFreezeOrg(orgId);
  const rank = useFreezeAuthority(orgId);
  return (
    <DangerZone
      id="freeze-org"
      title="Freeze this organization"
      description="Stops every agent of every workspace until an owner unfreezes it. Nothing is deleted."
    >
      <FreezeControl subject="org" name={name} frozen={frozen} callerRank={rank} freeze={m.freeze} unfreeze={m.unfreeze} />
    </DangerZone>
  );
}

/** Instance administration: freeze or lift an organization as the operator (rank 3). */
export function AdminOrgFreeze({ orgId, name, frozen }: { orgId: string; name: string; frozen: Frozen | null | undefined }) {
  const m = useAdminFreezeOrg(orgId);
  return <FreezeControl subject="org" name={name} frozen={frozen} callerRank={3} freeze={m.freeze} unfreeze={m.unfreeze} />;
}
