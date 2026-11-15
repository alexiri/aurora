"use client";

import { useState, type FormEvent, type ReactNode } from "react";

import { Button } from "@/components/ui/button";
import { FormDialog } from "@/components/ui/confirm-dialog";
import { Field, inputClass } from "@/components/ui/field";
import { Notice } from "@/components/ui/notice";
import { oidcStartUrl } from "@/lib/api/auth";
import { ApiError } from "@/lib/api/client";
import { errorMessage } from "@/lib/api/errors";
import { useInstance } from "@/lib/api/queries";

/**
 * The danger dialog of Admin › Organizations (P5-ADMDEL, docs/14 §Instance administration): names what
 * goes, asks for the slug typed exactly (submit stays disabled until it matches), and on 412
 * `reauth_required` offers the same provider re-authentication step as account deletion — the links return
 * to `returnTo`, where the person opens the dialog and confirms once more. The API stays authoritative:
 * the typed slug is checked there too (400 `confirmation_mismatch`).
 */
export function AdminDeleteDialog({
  open,
  onOpenChange,
  title,
  consequences,
  noun,
  slug,
  returnTo,
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
  /** Where the provider sign-in comes back to. */
  returnTo: string;
  /** Sends the delete with the typed slug; rejects with the API's error. */
  run: (confirm: string) => Promise<unknown>;
  onDone: () => void;
}) {
  const instance = useInstance();
  const [typed, setTyped] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<unknown>(null);
  const needsReauth = error instanceof ApiError && error.code === "reauth_required";
  const providers = instance.data?.oidc_providers ?? [];
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
        <Notice tone="danger" role="alert">
          {errorMessage(error)}
        </Notice>
      ) : null}
      {needsReauth ? (
        <div className="space-y-2" aria-label="Re-authenticate">
          {providers.map((p) => (
            <Button key={p.id} asChild variant="outline" className="w-full">
              {/* Full navigation: the API redirects to the provider with prompt=login. */}
              <a href={oidcStartUrl(p.id, { reauth: true, returnTo })}>Sign in again with {p.display_name}</a>
            </Button>
          ))}
          <p className="text-xs text-muted">After signing in you come back here; open the dialog again and confirm.</p>
        </div>
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
