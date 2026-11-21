"use client";

import { useAnswerFix } from "@/components/agents/answer-fix";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { ErrorNotice } from "@/components/ui/notice";
import { showUndoToast } from "@/components/ui/undo-toast";
import { useAddCredentialGrants } from "@/lib/api/credential-grants";

/**
 * (P5-REFINE-12, access-3, priority 10) The organization's starter AI key, when it is what this agent is missing: the agent
 * has no AI key at all and the answer check (C-1062 `fix`, the same source as the header's action) says `give_key`. One click
 * gives it. Words use the key's own name. Nothing shows when the agent can answer, when the fix is another one (add a key,
 * ask an admin), or once the key is given.
 */
export function useStarterKeyOffer(agentId: string, enabled: boolean) {
  const { noKey, fix } = useAnswerFix(agentId, enabled);
  const key = noKey && fix?.action === "give_key" ? fix.key : null;
  const add = useAddCredentialGrants({ kind: "secret", id: key?.secret_id ?? "" });
  return { offer: key ? { keyName: key.name } : null, add };
}

export function StarterKeyCard({ agentId, agentName }: { agentId: string; agentName: string }) {
  const { offer, add } = useStarterKeyOffer(agentId, true);
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
