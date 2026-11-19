"use client";

import { useState, type FormEvent, type ReactNode } from "react";

import { FormDialog } from "@/components/ui/confirm-dialog";
import { Field, inputClass } from "@/components/ui/field";
import { ErrorNotice } from "@/components/ui/notice";

/**
 * The danger dialog of Admin › Organizations (P5-ADMDEL, docs/14 §Instance administration): names what
 * goes and asks for the slug typed exactly (submit stays disabled until it matches); the first confirm
 * deletes, with no re-authentication step (phase 5, FIX-157, D-317). The API stays authoritative: the
 * typed slug is checked there too (400 `confirmation_mismatch`).
 */
export function AdminDeleteDialog({
  open,
  onOpenChange,
  title,
  consequences,
  noun,
  slug,
  run,
  onDone,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  title: string;
  consequences: ReactNode;
  /** "organization" or "workspace": the dialog's words. */
  noun: string;
  slug: string;
  /** Sends the delete with the typed slug; rejects with the API's error. */
  run: (confirm: string) => Promise<unknown>;
  onDone: () => void;
}) {
  const [typed, setTyped] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<unknown>(null);
  const inputId = `admin-delete-${noun}-confirm`;

  function reset() {
    setTyped("");
    setError(null);
    setBusy(false);
  }

  async function submit(e: FormEvent) {
    e.preventDefault();
    if (typed !== slug) return;
    setBusy(true);
    setError(null);
    try {
      await run(typed);
      reset();
      onOpenChange(false);
      onDone();
    } catch (err) {
      setError(err);
      setBusy(false);
    }
  }

  return (
    <FormDialog
      open={open}
      onOpenChange={(o) => {
        if (!o) reset();
        onOpenChange(o);
      }}
      title={title}
      submitLabel={busy ? "Deleting…" : `Delete ${noun}`}
      danger
      busy={busy}
      submitDisabled={typed !== slug}
      onSubmit={(e) => void submit(e)}
    >
      <div className="space-y-2 text-sm text-ink-2">{consequences}</div>
      {error !== null ? (
        <ErrorNotice error={error} />
      ) : null}
      <Field id={inputId} label={`Type ${slug} to confirm`}>
        <input
          id={inputId}
          className={`${inputClass} font-mono`}
          value={typed}
          onChange={(e) => setTyped(e.target.value)}
          autoComplete="off"
          autoCapitalize="off"
          spellCheck={false}
        />
      </Field>
    </FormDialog>
  );
}
