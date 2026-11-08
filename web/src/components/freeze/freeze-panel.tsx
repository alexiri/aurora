"use client";

import { useState, type FormEvent, type ReactNode } from "react";

import { Button } from "@/components/ui/button";
import { FormDialog } from "@/components/ui/confirm-dialog";
import { Field, inputClass } from "@/components/ui/field";
import { Notice } from "@/components/ui/notice";
import { errorMessage } from "@/lib/api/errors";
import { authorityRank, type Frozen } from "@/lib/api/freeze";

export type FreezeSubject = "agent" | "workspace" | "org";

const SUBJECT_LABEL: Record<FreezeSubject, string> = {
  agent: "agent",
  workspace: "workspace",
  org: "organization",
};

const REASON_MAX = 500;

function date(iso: string): string {
  return new Date(iso).toLocaleString();
}

/** "Frozen by <name> on <date>: <reason>" (docs/14 §Agent page, RFE-006). */
export function FrozenBanner({ frozen, subject, children }: { frozen: Frozen; subject: FreezeSubject; children?: ReactNode }) {
  const inherited = frozen.level !== subject;
  return (
    <Notice tone="danger" role="status" className="space-y-2 rounded-card border-danger-ink p-4">
      {inherited ? (
        <p className="font-semibold">
          This {SUBJECT_LABEL[subject]} is paused because its {SUBJECT_LABEL[frozen.level as FreezeSubject] ?? frozen.level} is frozen.
        </p>
      ) : null}
      <p>
        Frozen by {frozen.by_label} on {date(frozen.at)}: {frozen.reason}
      </p>
      {children}
    </Notice>
  );
}

interface FreezeMutation {
  mutateAsync: (reason: string) => Promise<unknown>;
  isPending: boolean;
  error: unknown;
}
interface UnfreezeMutation {
  mutateAsync: () => Promise<unknown>;
  isPending: boolean;
  error: unknown;
}

const EFFECTS = [
  "Every credential of the affected agents is refused and live connections are closed.",
  "Running and queued turns are stopped, and no new turn starts, from any door.",
  "Schedules, webhooks and chat presences pause; they resume when you unfreeze.",
  "The agent's computer is parked; its files and history are kept.",
  "Owners can still read transcripts and activity, change settings, and unfreeze.",
];

/**
 * Freeze / unfreeze control for one scope (agent, workspace or organization). `frozen` is the
 * freeze that stops the subject; the control offers Unfreeze only when it is set at this level and
 * the caller's authority reaches the one that froze it (`callerRank`, see `useFreezeAuthority`).
 */
export function FreezeControl({
  subject,
  name,
  frozen,
  callerRank,
  freeze,
  unfreeze,
}: {
  subject: FreezeSubject;
  name: string;
  frozen: Frozen | null | undefined;
  callerRank: number;
  freeze: FreezeMutation;
  unfreeze: UnfreezeMutation;
}) {
  const label = SUBJECT_LABEL[subject];
  const [open, setOpen] = useState(false);
  const [reason, setReason] = useState("");

  async function submit(e: FormEvent) {
    e.preventDefault();
    try {
      await freeze.mutateAsync(reason.trim());
      setOpen(false);
      setReason("");
    } catch {
      // surfaced through freeze.error
    }
  }

  if (frozen) {
    const here = frozen.level === subject;
    const tooLow = authorityRank(frozen.authority) > callerRank;
    return (
      <FrozenBanner frozen={frozen} subject={subject}>
        {here ? (
          <div className="space-y-1">
            <Button
              type="button"
              variant="outline"
              size="sm"
              disabled={unfreeze.isPending || tooLow}
              aria-describedby={tooLow ? `unfreeze-${subject}-why` : undefined}
              onClick={() => void unfreeze.mutateAsync().catch(() => undefined)}
            >
              {unfreeze.isPending ? "Unfreezing…" : `Unfreeze ${label}`}
            </Button>
            {tooLow ? (
              <p id={`unfreeze-${subject}-why`} className="text-xs">
                A higher authority froze this ({frozen.authority.replace("_", " ")}); only that authority or a higher one can unfreeze it.
              </p>
            ) : null}
          </div>
        ) : (
          <p className="text-xs">Unfreeze it at the {SUBJECT_LABEL[frozen.level as FreezeSubject] ?? frozen.level} level.</p>
        )}
        {unfreeze.error ? (
          <p role="alert" className="text-xs">
            {errorMessage(unfreeze.error)}
          </p>
        ) : null}
      </FrozenBanner>
    );
  }

  return (
    <>
      <Button type="button" variant="danger" size="sm" onClick={() => setOpen(true)}>
        Freeze {label}
      </Button>
      <FormDialog
        open={open}
        onOpenChange={setOpen}
        title={`Freeze ${label} ${name}?`}
        description="An emergency stop. Nothing is deleted and you can unfreeze at any time."
        submitLabel={freeze.isPending ? "Freezing…" : `Freeze ${label}`}
        danger
        busy={freeze.isPending}
        submitDisabled={reason.trim() === ""}
        onSubmit={(e) => void submit(e)}
      >
        <ul className="list-disc space-y-1 pl-5 text-sm text-ink-2">
          {EFFECTS.map((e) => (
            <li key={e}>{e}</li>
          ))}
        </ul>
        <Field id={`freeze-reason-${subject}`} label="Reason" hint={`Required, up to ${REASON_MAX} characters. Shown to everyone who can see the ${label}.`}>
          <textarea
            id={`freeze-reason-${subject}`}
            className={inputClass}
            rows={3}
            maxLength={REASON_MAX}
            required
            value={reason}
            onChange={(e) => setReason(e.target.value)}
          />
        </Field>
        {freeze.error ? (
          <Notice tone="danger" role="alert">
            {errorMessage(freeze.error)}
          </Notice>
        ) : null}
      </FormDialog>
    </>
  );
}
