"use client";

import Link from "next/link";

import { AccountsList } from "@/components/accounts/accounts-list";
import { StatusLine } from "@/components/ui/status-line";
import { useModelReadiness } from "@/lib/api/credential-grants";

import { AgentSummary } from "./agent-summary";
import { StarterKeyCard, useStarterKeyOffer } from "./starter-key-card";
import { REASON_COPY } from "./model-readiness";

/**
 * (P5-POLISH-6, D-385) A model-readiness problem as one line at the top of the AI key's place in the list: "Can't answer:
 * <reason>" with its fix link ("Choose a model" opens Instructions › Model). Nothing when the agent can answer; the
 * readiness block, roles and endpoint names are not on this page.
 */
function CantAnswerLine({ agentId, base, enabled }: { agentId: string; base: string; enabled: boolean }) {
  const readiness = useModelReadiness(agentId, enabled);
  // The starter-key card above says it (and gives the key): no second line for the same problem.
  const { offer } = useStarterKeyOffer(agentId, enabled);
  const d = readiness.data;
  if (!d || offer) return null;
  const usable = d.keys.some((k) => k.usable);
  if (usable && d.default) return null;
  const firstBad = d.keys.find((k) => !k.usable);
  const reason = d.keys.length === 0 ? "no AI key is given to this agent" : (firstBad?.reason ? REASON_COPY[firstBad.reason]?.text.replace(/\.$/, "") : null) ?? "the model is not usable";
  return (
    <StatusLine
      tone="fail"
      data-testid="cant-answer"
      action={
        <Link href={`${base}?tab=instructions#a-model`} className="text-sm font-medium text-link underline">
          Choose a model
        </Link>
      }>
      Can&apos;t answer: {reason}
    </StatusLine>
  );
}

/**
 * An agent's Access page (P5-UX-4, P5-POLISH-6, `14 §1`): what the agent can do (the summary), then **Apps and accounts** with
 * **Give <Agent> an app**, then the workspace's one list read for this agent. Accounts the agent can use come first, each
 * with its health and its access in the row, then "Not given to <Agent>" collapsed. Members read the same list as text.
 */
export function AccessTab({
  agentId,
  agentName,
  wsId,
  orgId,
  canOwn,
  hosted = false,
  base = "",
  agentBase,
}: {
  agentId: string;
  agentName: string;
  wsId: string;
  orgId: string;
  canOwn: boolean;
  /** Hosted agents show a model problem (owners only). */
  hosted?: boolean;
  /** `/o/{org}/w/{ws}`. */
  base?: string;
  /** The agent's own page, for the "Choose a model" link. */
  agentBase?: string;
}) {
  return (
    <div className="space-y-6">
      {/* (P5-REFINE-12, access-3) The organization's starter key, one click, when it is what this agent is missing. */}
      {hosted ? <StarterKeyCard agentId={agentId} agentName={agentName} /> : null}
      {/* The agent's summary (C-986): the same sentences as Welcome and the Overview, above the list they come from. */}
      <section aria-label={`What ${agentName} can do`} className="rounded-card border border-line bg-surface p-4 shadow-card sm:p-5">
        <h2 className="font-display text-base font-semibold text-ink">What {agentName} can do</h2>
        <AgentSummary wsId={wsId} orgId={orgId} agent={{ id: agentId, name: agentName }} className="mt-2 space-y-1 text-sm text-ink-2" />
      </section>
      {canOwn && hosted ? <CantAnswerLine agentId={agentId} base={agentBase ?? base} enabled /> : null}
      <AccountsList scope={{ kind: "workspace", id: wsId }} orgId={orgId} canOwn={canOwn} agent={{ id: agentId, name: agentName }} />
    </div>
  );
}
