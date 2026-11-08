"use client";

import { useParams } from "next/navigation";

import { useOrg, useResolve, useWorkspace } from "@/lib/api/queries";

import { FrozenBanner } from "./freeze-panel";

/**
 * Banner above the page content on every workspace and organization page while the workspace
 * (or its organization) is frozen (docs/14 §Shell, RFE-006). The workspace freeze reported by the
 * API already includes the organization's, with `level` saying where it is set.
 */
export function FrozenEntityBanner() {
  const { org, ws } = useParams<{ org?: string; ws?: string }>();
  const resolved = useResolve(org, ws);
  const wsId = resolved.data?.workspace?.id;
  const workspace = useWorkspace(wsId);
  const orgQuery = useOrg(resolved.data?.org.id);
  if (!org) return null;
  if (ws) {
    const f = workspace.data?.frozen;
    return f ? <FrozenBanner frozen={f} subject="workspace" /> : null;
  }
  const f = orgQuery.data?.frozen;
  return f ? <FrozenBanner frozen={f} subject="org" /> : null;
}
