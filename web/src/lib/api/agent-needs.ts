"use client";

import { useMemo } from "react";

import { usePendingActionApprovals } from "./action-approvals";
import { useCanApprove, usePendingApprovals } from "./approvals";

/**
 * (P5-REFINE-11 item 11, P5-REFINE-20 G5) How many approvals wait for the viewer on each agent of the workspace, by agent id:
 * the gateway holds and the action approvals, the same two lists the agent page's rail shows as "Needs you". Empty for a
 * person who cannot decide (no number is shown to them, no request is made). Polled (30 s and 10 s) and refetched by the
 * approvals stream where a page opens it; deciding anywhere refetches these lists, so the number drops without a reload.
 */
export function useAgentNeedsCounts(wsId: string | undefined): { counts: Record<string, number>; canDecide: boolean } {
  const canDecide = useCanApprove(wsId);
  const gateway = usePendingApprovals(wsId ?? "", canDecide, { poll: true });
  const actions = usePendingActionApprovals(canDecide ? (wsId ?? "") : "");
  const counts = useMemo(() => {
    const out: Record<string, number> = {};
    if (!canDecide) return out;
    const add = (id: string | undefined | null) => {
      if (id) out[id] = (out[id] ?? 0) + 1;
    };
    for (const a of gateway.data?.items ?? []) add(a.agent?.id);
    for (const a of actions.data?.items ?? []) add(a.agent_id);
    return out;
  }, [canDecide, gateway.data, actions.data]);
  return { counts, canDecide };
}
