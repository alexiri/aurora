"use client";

import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { ErrorNotice } from "@/components/ui/notice";
import { showUndoToast } from "@/components/ui/undo-toast";
import { useAccounts } from "@/lib/api/accounts";
import { useAddCredentialGrants, useModelReadiness } from "@/lib/api/credential-grants";
import { useOrg } from "@/lib/api/queries";

/**
 * (P5-REFINE-12, access-3, priority 10) The organization's starter AI key, when it is what this agent is missing: the
 * agent cannot answer, the organization has a starter key, and the agent was never given it (it was created before the
 * key was set). One click gives it. Words use the key's own name. Nothing shows when the agent can answer, when the
 * organization has no starter key, or once the key is given (P5-REFINE-5 gives a later-added starter key to existing
 * agents itself, so this card is then never needed).
 */
export function useStarterKeyOffer(agentId: string, orgId: string, enabled: boolean) {
  const readiness = useModelReadiness(agentId, enabled);
  // `GET /v1/orgs/{org}/accounts` is for owners and admins (403 for anyone else): a member is never asked (no console error).
  const org = useOrg(enabled ? orgId : undefined);
  const isAdmin = org.data?.my_role === "owner" || org.data?.my_role === "admin";
  const accounts = useAccounts({ kind: "organization", id: orgId }, enabled && isAdmin);
  const add = useAddCredentialGrants({ kind: "secret", id: accounts.data?.items.find((a) => a.kind === "ai_key" && a.starter)?.ref.id ?? "" });
  const d = readiness.data;
  const starter = accounts.data?.items.find((a) => a.kind === "ai_key" && a.starter === true);
  const canAnswer = !!d && d.keys.some((k) => k.usable) && !!d.default;
  const given = !!d && !!starter && d.keys.some((k) => k.secret_id === starter.ref.id);
  const offer = !!d && !!starter && !canAnswer && !given ? { keyName: starter.name } : null;
  return { offer, add };
}

export function StarterKeyCard({ agentId, agentName, orgId }: { agentId: string; agentName: string; orgId: string }) {
  const { offer, add } = useStarterKeyOffer(agentId, orgId, true);
  if (!offer) return null;
  return (
    <Card role="region" aria-label={`${agentName} has no AI key`} data-testid="starter-key-card" className="space-y-3">
      <h2 className="font-display text-base font-semibold text-ink">
        {agentName} can&apos;t answer yet: it has no AI key.
      </h2>
      <p className="text-sm text-ink-2">
        Your organization&apos;s {offer.keyName} goes to every new agent automatically, and {agentName} was created before it was set.
      </p>
      <ErrorNotice error={add.error} />
      <div className="flex flex-wrap gap-2">
        <Button
          type="button"
          disabled={add.isPending}
          onClick={() => add.mutate({ agentIds: [agentId] }, { onSuccess: () => showUndoToast(`${agentName} can answer now`) })}
        >
          Give {agentName} the {offer.keyName}
        </Button>
        <Button
          type="button"
          variant="outline"
          onClick={() => {
            const target = Array.from(document.querySelectorAll("button")).find((b) => b.textContent?.trim() === "Add an AI key");
            target?.scrollIntoView?.({ block: "center" });
            target?.focus();
          }}
        >
          Choose another
        </Button>
      </div>
    </Card>
  );
}
