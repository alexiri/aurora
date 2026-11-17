"use client";

import { AccountsList } from "@/components/accounts/accounts-list";

import { AgentSummary } from "./agent-summary";
import { ModelReadinessSection } from "./model-readiness";

/**
 * An agent's Access tab (P5-UX-4, `14 §1`): the workspace's one list of accounts, read for this agent. Accounts the
 * agent can use come first, each with its access shown and editable in the row (owners), then "Not given to <Agent>"
 * collapsed. Members read the same list as text. The Connections, Secrets and Effective access sections are gone: the
 * list is the whole answer to "what can this agent use, and what may it do there?".
 */
export function AccessTab({
  agentId,
  agentName,
  wsId,
  orgId,
  canOwn,
  hosted = false,
  base = "",
}: {
  agentId: string;
  agentName: string;
  wsId: string;
  orgId: string;
  canOwn: boolean;
  /** Hosted agents get the Model readiness section (owners only). */
  hosted?: boolean;
  /** `/o/{org}/w/{ws}`, for the links of the readiness fixes and the Apps tab. */
  base?: string;
}) {
  return (
    <div className="space-y-8">
      {/* The agent's summary (C-986): the same sentences as Welcome and the Overview, above the list they come from. */}
      <section aria-label={`What ${agentName} can do`} className="rounded-card border border-line bg-surface p-4 shadow-card sm:p-5">
        <h2 className="font-display text-base font-semibold text-ink">What {agentName} can do</h2>
        <AgentSummary wsId={wsId} orgId={orgId} agent={{ id: agentId, name: agentName }} canOwn={canOwn} className="mt-2 space-y-1 text-sm text-ink-2" />
      </section>
      {canOwn && hosted ? <ModelReadinessSection agentId={agentId} base={base} /> : null}
      <AccountsList
        scope={{ kind: "workspace", id: wsId }}
        orgId={orgId}
        canOwn={canOwn}
        agent={{ id: agentId, name: agentName }}
      />
    </div>
  );
}
