"use client";

import { useState } from "react";

import { ErrorState } from "@/components/states";
import { Button } from "@/components/ui/button";
import { ChoiceCards } from "@/components/ui/choice-cards";
import { Dialog, DialogClose, DialogContent } from "@/components/ui/dialog";
import { Notice } from "@/components/ui/notice";
import { LEVEL_LABEL, LEVEL_ORDER, loweringSentences } from "@/lib/accounts";
import { accountKeys, type Account, type Level } from "@/lib/api/accounts";
import { useLimitPreview, usePatchConnection } from "@/lib/api/apps";
import { useQueryClient } from "@tanstack/react-query";

/** What each limit means for the agents that can use the account (`14 §2`, the limit). */
const LIMIT_SENTENCE: Record<Level, string> = {
  read: "No agent can change anything with this account.",
  ask: "Agents ask you before each change. No agent can change anything without asking.",
  act: "Agents may be given every choice, up to changing without asking.",
};

/**
 * The limit of an organization's account (C-937, C-990): "Agents may at most: <choice>", opening the three choices.
 * Lowering it shows who goes down (the preview) before it is saved; raising it changes no agent. Shown to organization
 * owners and admins only; everyone else reads the limit in the row's access choices.
 */
export function LimitControl({ account, orgId }: { account: Account; orgId: string }) {
  const current: Level | null = account.limit ?? null;
  const [open, setOpen] = useState(false);
  const [pick, setPick] = useState<Level | null>(null);
  const qc = useQueryClient();
  const patch = usePatchConnection({ kind: "organization", id: orgId });
  const lowering = pick !== null && current !== null && LEVEL_ORDER.indexOf(pick) < LEVEL_ORDER.indexOf(current);
  const preview = useLimitPreview(account.ref.id, lowering ? pick : null);
  if (current === null || account.ref.type !== "connection") return null;
  const choices = LEVEL_ORDER.filter((l) => !(l === "act" && account.app?.custom_only));
  const selected: Level = pick ?? current;
  const lowered = preview.data?.lowered ?? [];
  const sentences = loweringSentences(lowered);

  function close() {
    setOpen(false);
    setPick(null);
    patch.reset();
  }

  return (
    <>
      <p className="flex flex-wrap items-center gap-2 text-sm text-ink-2">
        <span>
          Agents may at most: <span className="font-medium text-ink">{LEVEL_LABEL[current]}</span>
        </span>
        <Button size="sm" variant="outline" aria-label={`Change the limit of ${account.name}`} onClick={() => setOpen(true)}>
          Change
        </Button>
      </p>
      <Dialog open={open} onOpenChange={(o) => (o ? setOpen(true) : close())}>
        <DialogContent title={`Limit for ${account.name}`} description="No agent in any workspace can have more access than this.">
          <div className="space-y-3">
            <ChoiceCards<Level>
              legend="Agents may at most:"
              value={selected}
              onChange={setPick}
              options={choices.map((l) => ({ value: l, label: LEVEL_LABEL[l], description: LIMIT_SENTENCE[l] }))}
            />
            {lowering ? (
              preview.isPending ? (
                <p className="text-sm text-muted" role="status">
                  Checking which agents change…
                </p>
              ) : preview.isError ? (
                <ErrorState error={preview.error} />
              ) : sentences.length > 0 ? (
                <Notice tone="pending" role="status">
                  <p className="font-medium text-ink">Lowering the limit changes agents that are above it:</p>
                  <ul className="list-disc pl-5">
                    {sentences.map((s) => (
                      <li key={s}>{s}</li>
                    ))}
                  </ul>
                </Notice>
              ) : (
                <p className="text-sm text-ink-2" role="status">
                  No agent changes.
                </p>
              )
            ) : null}
            {patch.error ? <ErrorState error={patch.error} /> : null}
            <div className="flex flex-wrap justify-end gap-2">
              <DialogClose asChild>
                <Button variant="outline" type="button">
                  Cancel
                </Button>
              </DialogClose>
              <Button
                type="button"
                disabled={pick === null || pick === current || patch.isPending || (lowering && !preview.isSuccess)}
                onClick={() => {
                  if (pick === null) return;
                  patch.mutate(
                    { id: account.ref.id, limit: pick },
                    {
                      onSuccess: () => {
                        void qc.invalidateQueries({ queryKey: accountKeys.all });
                        close();
                      },
                    },
                  );
                }}
              >
                {patch.isPending ? "Saving…" : lowering && lowered.length > 0 ? `Lower to ${LEVEL_LABEL[selected]}` : "Save"}
              </Button>
            </div>
          </div>
        </DialogContent>
      </Dialog>
    </>
  );
}
