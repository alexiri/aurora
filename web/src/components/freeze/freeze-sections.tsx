"use client";

import {
  useAdminFreezeOrg,
  useFreezeAgent,
  useFreezeAuthority,
  useFreezeOrg,
  useFreezeWorkspace,
  type Frozen,
} from "@/lib/api/freeze";

import { FreezeControl, FrozenBanner } from "./freeze-panel";

/** Agent header: banner when frozen (for everyone), Freeze / Unfreeze for `can_own`. */
export function AgentFreeze({
  agentId,
  name,
  orgId,
  frozen,
  canOwn,
}: {
  agentId: string;
  name: string;
  orgId: string;
  frozen: Frozen | null | undefined;
  canOwn: boolean;
}) {
  const m = useFreezeAgent(agentId);
  const rank = useFreezeAuthority(orgId);
  // A workspace or organization freeze is shown by the shell banner (frozen-entity-banner).
  if (frozen && frozen.level !== "agent") return null;
  if (!canOwn) return frozen ? <FrozenBanner frozen={frozen} subject="agent" /> : null;
  return <FreezeControl subject="agent" name={name} frozen={frozen} callerRank={rank} freeze={m.freeze} unfreeze={m.unfreeze} />;
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
    <section aria-label="Freeze workspace" className="max-w-md space-y-3">
      <h2 className="text-lg font-medium">Freeze this workspace</h2>
      <p className="text-sm text-ink-2">
        Stops every agent in the workspace until you unfreeze it. Nothing is deleted.
      </p>
      <FreezeControl subject="workspace" name={name} frozen={frozen} callerRank={rank} freeze={m.freeze} unfreeze={m.unfreeze} />
    </section>
  );
}

/** Organization settings, danger zone (org owners). */
export function OrgFreeze({ orgId, name, frozen }: { orgId: string; name: string; frozen: Frozen | null | undefined }) {
  const m = useFreezeOrg(orgId);
  const rank = useFreezeAuthority(orgId);
  return (
    <section aria-label="Freeze organization" className="max-w-md space-y-3">
      <h2 className="text-lg font-medium">Freeze this organization</h2>
      <p className="text-sm text-ink-2">
        Stops every agent of every workspace until an owner unfreezes it. Nothing is deleted.
      </p>
      <FreezeControl subject="org" name={name} frozen={frozen} callerRank={rank} freeze={m.freeze} unfreeze={m.unfreeze} />
    </section>
  );
}

/** Instance administration: freeze or lift an organization as the operator (rank 3). */
export function AdminOrgFreeze({ orgId, name, frozen }: { orgId: string; name: string; frozen: Frozen | null | undefined }) {
  const m = useAdminFreezeOrg(orgId);
  return <FreezeControl subject="org" name={name} frozen={frozen} callerRank={3} freeze={m.freeze} unfreeze={m.unfreeze} />;
}
