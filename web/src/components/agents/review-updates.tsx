"use client";

import { useEffect, useState } from "react";

import { ReviewChangesDialog } from "@/components/templates/review-changes-dialog";
import { Button } from "@/components/ui/button";
import { Dialog, DialogClose, DialogContent } from "@/components/ui/dialog";
import {
  agentTemplateKeys,
  useAgentTemplate,
} from "@/lib/api/agent-templates";
import { useQueryClient } from "@tanstack/react-query";

/**
 * (FIX-251, D-488(a)) **Review updates**, the Agents page's walkthrough: each agent that needs an
 * update, one review at a time in the order given. An agent whose offer is gone by the time it is
 * read (applied elsewhere, the template withdrawn) is skipped; closing a review — applied, skipped
 * or stale — moves to the next agent, and the last close ends the walkthrough. Every offer is read
 * fresh from the agent's own template status, so what each review shows is what an apply checks.
 */
export function ReviewUpdates({
  agents,
  agentBaseOf,
  onClose,
}: {
  agents: ReadonlyArray<{ id: string; name: string; handle: string }>;
  /** The per-agent base the review's **Give access** links open (the agent's Access tab). */
  agentBaseOf: (handle: string) => string;
  /** The walkthrough is over: every agent given has been reviewed or skipped. */
  onClose: () => void;
}) {
  const [index, setIndex] = useState(0);
  const done = () => setIndex((i) => i + 1);
  const agent = agents[index];
  useEffect(() => {
    if (index >= agents.length) onClose();
  }, [index, agents.length, onClose]);
  if (!agent) return null;
  return (
    <ReviewUpdatesStep
      key={agent.id}
      agent={agent}
      agentBase={agentBaseOf(agent.handle)}
      remaining={agents.length - index}
      onDone={done}
      onStop={onClose}
    />
  );
}

/** One step: the agent's offer, read fresh; the review dialog when there is one, a skip otherwise. */
function ReviewUpdatesStep({
  agent,
  agentBase,
  remaining,
  onDone,
  onStop,
}: {
  agent: { id: string; name: string; handle: string };
  agentBase: string;
  /** How many agents are still to review, this one included. */
  remaining: number;
  onDone: () => void;
  /** The person stops the walkthrough (while an offer is still being read). */
  onStop: () => void;
}) {
  const status = useAgentTemplate(agent.id);
  const qc = useQueryClient();
  const update = status.data?.update ?? null;
  // Nothing to review here (the offer is gone, or the status failed to read): the step is over.
  useEffect(() => {
    if (status.isError || (status.isSuccess && !update)) onDone();
  }, [status.isError, status.isSuccess, update, onDone]);
  // (C-1112) A 409 `template_version_changed`: the offer moved on while the person reviewed.
  // The next read answers with the fresh one; this agent's review is over for now.
  const onStale = () => {
    void qc.invalidateQueries({ queryKey: agentTemplateKeys.status(agent.id) });
    onDone();
  };
  if (status.isPending || !update) {
    return (
      <Dialog open>
        <DialogContent
          title="Review updates"
          description={`Reading the update for ${agent.name} — ${remaining} to review.`}
          size="sm"
          footer={
            <DialogClose asChild>
              <Button variant="outline" onClick={onStop}>
                Stop for now
              </Button>
            </DialogClose>
          }
        >
          <p className="text-sm text-ink-2" data-testid="review-updates-reading">
            {status.isError ? "The update could not be read just now." : `Reading ${agent.name}'s update…`}
          </p>
        </DialogContent>
      </Dialog>
    );
  }
  return (
    <ReviewChangesDialog
      agentId={agent.id}
      agentName={agent.name}
      templateName={status.data?.template?.name ?? ""}
      offer={update}
      agentBase={agentBase}
      onStale={onStale}
      onClose={onDone}
    />
  );
}
