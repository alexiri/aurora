import type { TimelineItem } from "@/lib/api/timeline";

/**
 * (P5-REFINE-11, -16) Where a timeline row leads: the page of the thing it is about. `base` is the workspace's path
 * (`/o/x/w/y`), `agentBase` the agent's page (`${base}/agents/${handle}`). `null`: the row has no target.
 */
export function timelineHref(item: TimelineItem, { base, agentBase }: { base: string; agentBase: string }): string | null {
  const l = item.link;
  if (!l) return null;
  switch (l.type) {
    case "conversation":
      return `${agentBase}?c=${encodeURIComponent(l.id)}${l.seq != null ? `&m=${l.seq}` : ""}`;
    case "approval":
    case "action_approval":
      return `${base}/approvals`;
    case "run":
      return `${agentBase}?tab=schedules`;
    case "delivery":
      return `${agentBase}?tab=webhooks`;
    case "file":
      return l.path ? `${agentBase}?tab=files&path=${encodeURIComponent(l.path)}` : `${agentBase}?tab=files`;
    case "rule":
      return `${base}/internet-access`;
    case "app":
      return `${base}/apps`;
    case "peer_task":
      return `${agentBase}?tab=peers`;
    default:
      return null;
  }
}
