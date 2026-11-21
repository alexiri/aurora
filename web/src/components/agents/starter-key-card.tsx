"use client";

import { useAnswerFix } from "@/components/agents/answer-fix";
import { Button } from "@/components/ui/button";
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
    // (access-3 mockup) The amber callout with its dot: the sentence on the left, the two actions on the right.
    <section
      aria-label={`${agentName} has no AI key`}
      data-testid="starter-key-card"
      className="flex flex-wrap items-center gap-x-4 gap-y-3 rounded-card border border-pending-line bg-pending-panel p-4 text-pending-ink sm:p-5"
    >
      {/* The dot and the sentence wrap as one item, so the dot stays beside the first line. */}
      <div className="flex min-w-0 flex-1 basis-60 items-start gap-3">
        <span aria-hidden="true" className="mt-1.5 inline-block size-2 shrink-0 rounded-full bg-pending-ink" />
        <p className="min-w-0 flex-1 text-sm">
          <strong className="font-semibold">
            {agentName} can&apos;t answer yet: it has no AI key.
          </strong>{" "}
          Your organization&apos;s {offer.keyName} goes to every new agent automatically. {agentName} doesn&apos;t have it.
        </p>
      </div>
      <ErrorNotice error={add.error} className="basis-full" />
      <div className="flex min-w-0 max-w-full flex-wrap items-center gap-2">
        {/* (batch 10) The key's name can be long: the button wraps its words inside the card instead of running past it. */}
        <Button
          type="button"
          className="h-auto max-w-full whitespace-normal py-2 text-center"
          disabled={add.isPending}
          onClick={() => add.mutate({ agentIds: [agentId] }, { onSuccess: () => showUndoToast(`${agentName} can answer now`) })}
        >
          Give {agentName} the {offer.keyName}
        </Button>
        <Button
          type="button"
          variant="ghost"
          onClick={() => {
            const target = Array.from(document.querySelectorAll("button")).find((b) => b.textContent?.trim() === "Add an AI key");
            target?.scrollIntoView?.({ block: "center" });
            target?.focus();
          }}
        >
          Choose another
        </Button>
      </div>
    </section>
  );
}
