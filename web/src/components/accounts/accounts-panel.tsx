"use client";

import { useSearchParams } from "next/navigation";

import { ProposalsList } from "@/components/credentials/proposals";
import { ErrorState, Loading } from "@/components/states";
import { Section } from "@/components/ui/section";
import { useSecrets, type SecretScope } from "@/lib/api/secrets";

import { AccountsList } from "./accounts-list";

/**
 * The Accounts tab of a workspace or an organization (P5-IA-1, P5-UX-4): the one list of apps, websites and AI keys
 * (C-917, C-925). `kind=` (`app`, `website`, `ai-key`) shows one kind only; in a workspace, agents' requests for an
 * account come last (`#requests`) until the requests banner (P5-UX-17) takes the slot above the list.
 */
export function AccountsPanel({ scope, orgId, canOwn }: { scope: SecretScope; orgId: string; canOwn: boolean }) {
  const param = useSearchParams().get("kind");
  const kind = param === "app" || param === "website" ? param : param === "ai-key" ? "ai_key" : null;
  const secrets = useSecrets(scope, scope.kind === "workspace" && canOwn);
  return (
    <div className="space-y-8">
      <AccountsList scope={scope} orgId={orgId} canOwn={canOwn} kind={kind} />
      {scope.kind === "workspace" && canOwn && !kind ? (
        <Section id="requests" title="Requests" description="Accounts that agents asked for.">
          {secrets.isPending ? (
            <Loading />
          ) : secrets.isError ? (
            <ErrorState error={secrets.error} />
          ) : (
            <ProposalsList scope={scope} items={secrets.data?.items ?? []} canOwn={canOwn} />
          )}
        </Section>
      ) : null}
    </div>
  );
}
