import Link from "next/link";

import { Avatar } from "@/components/agent-avatar";
import { TokenBadges } from "@/components/agents/token-badges";
import { AttentionChip } from "@/components/ui/attention-chip";
import { Badge } from "@/components/ui/badge";
import { buttonVariants } from "@/components/ui/button";
import { agentChip } from "@/lib/agent-chip";
import type { Agent } from "@/lib/api/queries";

import { AgentSummary } from "./agent-summary";

/**
 * (P5-LAYOUT-3, D-440) One agent as a card in the `CardGrid`, the same frame as Apps and Accounts. Head: avatar, the name
 * as the link to the agent's page, `@handle`, and the needs-you chip when approvals wait. Then the badges, the summary
 * (C-986) when `wsId` and `orgId` are given, and **Chat** at the foot when `chat` is set. An `<li>`.
 */
export function AgentCard({
  agent: a,
  href,
  needs = 0,
  showTokenBadges = false,
  wsId,
  orgId,
  chat = false,
}: {
  agent: Agent;
  /** The agent's page; without it the name is plain text. */
  href?: string;
  /** Approvals waiting for the viewer on this agent; no chip at zero. */
  needs?: number;
  showTokenBadges?: boolean;
  wsId?: string;
  orgId?: string;
  chat?: boolean;
}) {
  const chip = agentChip(a);
  const byoOff = a.byo?.posture === "disabled";
  const token = showTokenBadges ? <TokenBadges token={a.byo?.token} /> : null;
  return (
    <li className="flex h-full min-w-0 flex-col gap-3 rounded-card border border-line bg-surface p-4 shadow-card sm:p-5">
      <div className="flex items-start gap-3">
        <Avatar name={a.name} url={a.avatar_url} tintKey={a.id} size={40} />
        <div className="min-w-0 flex-1">
          {href ? (
            <Link id={`agent-link-${a.id}`} href={href} className="block truncate font-semibold text-ink underline-offset-2 hover:underline">
              {a.name}
            </Link>
          ) : (
            <span className="block truncate font-semibold text-ink">{a.name}</span>
          )}
          <span className="block truncate font-mono text-sm text-muted">@{a.handle}</span>
        </div>
        {href && needs > 0 ? (
          <AttentionChip actionId={`agent-link-${a.id}`} data-testid="agent-needs-count" role="img" aria-label={`${needs} ${needs === 1 ? "approval" : "approvals"} waiting for you`}>
            <span aria-hidden="true">{needs}</span>
          </AttentionChip>
        ) : null}
      </div>
      {chip || byoOff || token || a.needs_update ? (
        <div className="flex flex-wrap items-center gap-2">
          {chip ? <Badge tone={chip.tone}>{chip.text}</Badge> : null}
          {/* (FIX-251, D-488(a)) The agent's template offers a newer version. */}
          {a.needs_update ? (
            <Badge tone="accent" data-testid="agent-needs-update">
              Update available
            </Badge>
          ) : null}
          {byoOff ? (
            <Badge tone="danger" title="Its tokens are refused until the setting is loosened; nothing was deleted.">
              BYO disabled by {a.byo?.posture_level}
            </Badge>
          ) : null}
          {token}
        </div>
      ) : null}
      {wsId && orgId ? <AgentSummary wsId={wsId} orgId={orgId} agent={a} className="space-y-0.5 text-sm text-ink-2" /> : null}
      {chat && href ? (
        <div className="mt-auto pt-1">
          <Link href={href} className={buttonVariants({ variant: "outline", size: "sm" })} aria-label={`Chat with ${a.name}`}>
            Chat
          </Link>
        </div>
      ) : null}
    </li>
  );
}
