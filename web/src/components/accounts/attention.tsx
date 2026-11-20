"use client";

import Link from "next/link";

import { Notice } from "@/components/ui/notice";
import { WhoCan } from "@/components/ui/who-can";
import { useAccounts, type Account } from "@/lib/api/accounts";
import { useWhoCan } from "@/lib/api/who-can";
import { attentionCount, healthClause, healthLine, needsAttention } from "@/lib/account-health";
/**
 * The accounts one agent can use that have stopped working (`14 §7`, C-945): given to the agent, and the app refuses
 * them, their sign-in expired, or their host cannot be reached. Read from the accounts list the Access tab reads.
 */
export function useAgentAttention(workspaceId: string | undefined, agentId: string): Account[] {
  const q = useAccounts({ kind: "workspace", id: workspaceId ?? "", agentId }, !!workspaceId);
  return (q.data?.items ?? []).filter((a) => a.given === true && needsAttention(a));
}

/** "1 account needs attention" on the agent's header, linking to the Access tab. Nothing when all is well. */
export function AgentAttentionChip({ workspaceId, agentId, accessHref }: { workspaceId: string; agentId: string; accessHref: string }) {
  const text = attentionCount(useAgentAttention(workspaceId, agentId).length);
  if (!text) return null;
  return (
    <Link href={accessHref} className="min-w-0 truncate whitespace-nowrap text-sm font-medium text-danger-ink underline">
      {text}
    </Link>
  );
}

/**
 * The banner above the message box of the agent's conversations: "Daneel can't use GitHub: the sign-in expired.
 * **Reconnect**". The fix is for people who can make it; everyone else sees who can.
 */
export function AccountsBanner({ workspaceId, agent, base }: { workspaceId: string; agent: { id: string; name: string }; base: string }) {
  const broken = useAgentAttention(workspaceId, agent.id);
  const who = useWhoCan({ workspaceId }, "workspace_owner", "manage_account");
  if (broken.length === 0) return null;
  return (
    <ul className="space-y-2" aria-label="Accounts that need attention">
      {broken.map((a) => {
        const fix = healthLine(a).fix;
        const can = a.can.manage;
        const href =
          fix?.kind === "reconnect" ? `${base}/agents/${agent.id}?tab=access&reconnect=${a.ref.id}` : `${base}/agents/${agent.id}?tab=access`;
        return (
          <li key={a.ref.id}>
            <Notice tone="pending" role="status">
              <span>
                {agent.name} can&apos;t use {a.app?.name ?? a.name}: {healthClause(a)}.{" "}
              </span>
              {fix && can ? (
                <Link href={href} className="font-medium underline">
                  {fix.label}
                </Link>
              ) : (
                <WhoCan value={who} />
              )}
            </Notice>
          </li>
        );
      })}
    </ul>
  );
}
