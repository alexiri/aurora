"use client";

import Link from "next/link";
import { useMemo } from "react";

import { Loading, ErrorState } from "@/components/states";
import { useAccounts } from "@/lib/api/accounts";
import { useEffectiveGovernance } from "@/lib/api/governance";
import { useInternetAccess } from "@/lib/api/policy";
import { agentSummary, LIMIT_THAT } from "@/lib/summary/agent";
import { internetAccessOf, type InternetAccess } from "@/lib/summary/internet";

/**
 * The workspace's Internet access for the sentences (C-986): the **published** rules and defaults, which every member of
 * the workspace reads (FIX-164, C-1020), and the limits above, which every member reads (effective governance).
 * `access` is `null` while they load or when either cannot be read; the summary then says nothing about websites rather
 * than guess.
 */
export function useWorkspaceInternet(wsId: string, orgId: string): { access: InternetAccess | null; pending: boolean } {
  const published = useInternetAccess(wsId);
  const eff = useEffectiveGovernance(orgId);
  const access = useMemo(() => {
    if (!published.data || !eff.data) return null;
    const e = eff.data.egress;
    return internetAccessOf(published.data, e.posture.value === "credentialed_only", e.baseline_allowlist.length + e.org_allowlist.length);
  }, [published.data, eff.data]);
  return { access, pending: published.isPending || eff.isPending };
}

/**
 * One agent's summary of what it can do (`14 §8`, C-986, D-350): the same sentences on Welcome, at the top of the agent's
 * Access tab and on the workspace Overview. Computed from real state, nothing stored.
 */
export function AgentSummary({
  wsId,
  orgId,
  agent,
  className,
  welcome,
}: {
  wsId: string;
  orgId: string;
  agent: { id: string; name: string };
  className?: string;
  /** Welcome's variant of the reach clause (firstday-5): `limitHref` is Internet access, for owners only. */
  welcome?: { limitHref?: string };
}) {
  const accounts = useAccounts({ kind: "workspace", id: wsId, agentId: agent.id });
  const internet = useWorkspaceInternet(wsId, orgId);
  const sentences = useMemo(
    () => (accounts.data ? agentSummary({ agent: agent.name, agentId: agent.id, accounts: accounts.data.items, internet: internet.access, welcome: welcome ? { canLimit: !!welcome.limitHref } : undefined }) : []),
    [accounts.data, agent.id, agent.name, internet.access, welcome],
  );
  if (accounts.isError) return <ErrorState error={accounts.error} onRetry={() => void accounts.refetch()} />;
  if (accounts.isPending || internet.pending) return <Loading />;
  return (
    <div className={className} data-testid="agent-summary">
      <p>{welcome?.limitHref ? withLimitLink(sentences.join(" "), welcome.limitHref) : sentences.join(" ")}</p>
    </div>
  );
}

/** Welcome: "you can limit that" is a link to Internet access. */
function withLimitLink(text: string, href: string) {
  const at = text.indexOf(LIMIT_THAT);
  if (at < 0) return text;
  return (
    <>
      {text.slice(0, at)}
      <Link href={href} className="font-medium text-link underline">
        {LIMIT_THAT}
      </Link>
      {text.slice(at + LIMIT_THAT.length)}
    </>
  );
}
