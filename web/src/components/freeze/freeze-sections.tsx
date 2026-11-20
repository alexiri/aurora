"use client";

import type { ReactNode } from "react";

import {
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
  menuExtra,
  menuLead,
  menuTriggerClassName,
  buttonLabel,
  buttonVariant,
}: {
  /** (P5-REFINE-11) The Pause button's words and look when it is a button in the Settings danger zone. */
  buttonLabel?: string;
  buttonVariant?: "danger" | "outline";
  menuLead?: ReactNode;
  menuTriggerClassName?: string;
  agentId: string;
  name: string;
  orgId: string;
  frozen: Frozen | null | undefined;
  canOwn: boolean;
  /** Compact header: Freeze is an overflow-menu entry (the banner and Unfreeze are unchanged). */
  inMenu?: boolean;
  /** More entries of that menu (the agent page's "Delete agent…"). */
  menuExtra?: ReactNode;
}) {
  const m = useFreezeAgent(agentId);
  const rank = useFreezeAuthority(orgId);
  // A workspace or organization freeze is shown by the shell banner (frozen-entity-banner).
  if (frozen && frozen.level !== "agent") return null;
  if (!canOwn) return frozen ? <FrozenBanner frozen={frozen} subject="agent" /> : null;
  return <FreezeControl subject="agent" name={name} frozen={frozen} callerRank={rank} freeze={m.freeze} unfreeze={m.unfreeze} inMenu={inMenu} menuExtra={menuExtra} menuLead={menuLead} menuTriggerClassName={menuTriggerClassName} buttonLabel={buttonLabel} buttonVariant={buttonVariant} />;
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
      title="Pause this workspace"
      description="Stops every agent in the workspace until you resume it. Nothing is deleted."
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
      title="Pause this organization"
      description="Stops every agent of every workspace until an owner resumes it. Nothing is deleted."
    >
      <FreezeControl subject="org" name={name} frozen={frozen} callerRank={rank} freeze={m.freeze} unfreeze={m.unfreeze} />
    </DangerZone>
  );
}
