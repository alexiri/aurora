"use client";

import { AccountsList } from "@/components/accounts/accounts-list";

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
