"use client";

import { useAttentionCounts } from "./attention";

/** One level's number for a rail: the total and the open items per topic (`topics`, C-1082). */
export interface LevelBadge {
  count: number;
  topics: Record<string, number>;
}

/**
 * (P5-REFINE-18 #18, C-1082) What waits in Administration for an instance admin: the total for the Overview entry and the
 * sidebar link, and a part per topic (`apps`, `chat-platforms`, `people`). `null` for everyone else and while loading or
 * when the read failed: a badge never shows an error.
 */
export function useAdminBadge(enabled = true): LevelBadge | null {
  const q = useAttentionCounts({}, enabled);
  if (!enabled || q.isError) return null;
  return q.data?.administration ?? null;
}

/**
 * (P5-REFINE-17 #17, C-1082) What waits in one organization for its owners and admins (requests for apps, chat platforms
 * and AI keys): the total for the Manage entry and the Overview, and a part per topic. `null` for everyone else.
 */
export function useOrgBadge(orgId: string | undefined, enabled = true): LevelBadge | null {
  const on = enabled && !!orgId;
  const q = useAttentionCounts({ org: orgId }, on);
  if (!on || q.isError) return null;
  return q.data?.organization ?? null;
}
