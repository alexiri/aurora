"use client";

import { useQueryClient } from "@tanstack/react-query";
import { useState, type FormEvent } from "react";

import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogTrigger } from "@/components/ui/dialog";
import { Field, inputClass } from "@/components/ui/field";
import { ApiError } from "@/lib/api/client";
import { chatApi, exportUrl } from "@/lib/api/conversations";

/** The same words the server uses, shown for the failures a person can act on. */
export function lifecycleError(err: unknown): string {
  if (err instanceof ApiError) {
    if (err.code === "conversation_active") return "A turn is still running. Stop it first, then delete.";
    if (err.status === 403) return "You are not allowed to do that to this conversation.";
    if (err.code === "export_too_large") return "This conversation is too large to export at once.";
    return err.message;
  }
  return "Something went wrong.";
}

export const CONFIRM_WORD = "delete";

/** Per-conversation menu (RFE-010): archive/unarchive, export, delete with a typed confirmation. */
export function ConversationActions({
  agentId,
  convId,
  archived,
  onGone,
}: {
  agentId: string;
  convId: string;
  archived: boolean;
  /** The conversation no longer exists (deleted): the parent picks another. */
  onGone: () => void;
}) {
  const qc = useQueryClient();
  const [open, setOpen] = useState(false);
  const [typed, setTyped] = useState("");
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<string | null>(null);

  const refresh = () => {
    void qc.invalidateQueries({ queryKey: ["agents", agentId, "conversations"] });
    void qc.invalidateQueries({ queryKey: ["agents", agentId, "direct"] });
  };

  async function run(fn: () => Promise<unknown>, after?: () => void) {
    setBusy(true);
    setErr(null);
    try {
      await fn();
      refresh();
      after?.();
    } catch (e) {
      setErr(lifecycleError(e));
    } finally {
      setBusy(false);
    }
  }

  function confirmDelete(e: FormEvent) {
    e.preventDefault();
    if (typed.trim().toLowerCase() !== CONFIRM_WORD) return;
    void run(
      () => chatApi.remove(convId),
      () => {
        setOpen(false);
        onGone();
      },
    );
  }

  return (
    <div role="group" aria-label="Conversation actions" className="flex flex-wrap items-center gap-2 text-sm">
      <Button
        size="sm"
        variant="outline"
        disabled={busy}
        onClick={() => void run(() => (archived ? chatApi.unarchive(convId) : chatApi.archive(convId)), archived ? undefined : onGone)}>
        {archived ? "Unarchive" : "Archive"}
      </Button>
      <a className="underline" href={exportUrl(convId, "json")} download>
        Export JSON
      </a>
      <a className="underline" href={exportUrl(convId, "zip")} download>
        Export with files (zip)
      </a>
      <Dialog
        open={open}
        onOpenChange={(o) => {
          setOpen(o);
          setTyped("");
          setErr(null);
        }}>
        <DialogTrigger asChild>
          <Button size="sm" variant="outline">
            Delete
          </Button>
        </DialogTrigger>
        <DialogContent
          title="Delete this conversation?"
          description="This permanently deletes the messages, attachments and the agent's stored session for this conversation. It cannot be undone.">
          <form onSubmit={confirmDelete} className="space-y-3">
            <p className="text-sm text-ink-2">
              Messages already posted to a chat platform, memories or skills the agent derived from this conversation, and exports you already downloaded are
              not deleted.
            </p>
            <Field id="delete-confirm" label={`Type "${CONFIRM_WORD}" to confirm`}>
              <input id="delete-confirm" className={inputClass} autoComplete="off" value={typed} onChange={(e) => setTyped(e.target.value)} />
            </Field>
            {err ? (
              <p role="alert" className="text-sm text-danger-ink">
                {err}
              </p>
            ) : null}
            <Button type="submit" variant="danger" disabled={busy || typed.trim().toLowerCase() !== CONFIRM_WORD}>
              {busy ? "Deleting…" : "Delete conversation"}
            </Button>
          </form>
        </DialogContent>
      </Dialog>
      {err && !open ? (
        <p role="alert" className="w-full text-danger-ink">
          {err}
        </p>
      ) : null}
    </div>
  );
}

/** Per-message "Redact": replaces the content in place (irreversible); optionally resets the session. */
export function RedactButton({ convId, seq }: { convId: string; seq: number }) {
  const [open, setOpen] = useState(false);
  const [reason, setReason] = useState("");
  const [reset, setReset] = useState(false);
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<string | null>(null);

  async function submit(e: FormEvent) {
    e.preventDefault();
    setBusy(true);
    setErr(null);
    try {
      const body: { reason?: string; reset_session?: boolean } = { reset_session: reset };
      if (reason.trim()) body.reason = reason.trim();
      await chatApi.redact(convId, seq, body);
      // The event refreshes through the stream's `event.redacted`; close the dialog.
      setOpen(false);
    } catch (e2) {
      setErr(lifecycleError(e2));
    } finally {
      setBusy(false);
    }
  }

  return (
    <Dialog
      open={open}
      onOpenChange={(o) => {
        setOpen(o);
        setErr(null);
      }}>
      <DialogTrigger asChild>
        <button type="button" className="text-xs text-muted underline" aria-label={`Redact message ${seq}`}>
          Redact
        </button>
      </DialogTrigger>
      <DialogContent title="Redact this message?" description="The content is replaced for everyone and cannot be restored.">
        <form onSubmit={(e) => void submit(e)} className="space-y-3">
          <p role="note" className="rounded-control border border-pending-line bg-pending-panel p-2 text-sm text-pending-ink">
            This is irreversible. The agent may still remember what it read; tick the box below to start it with a fresh session.
          </p>
          <Field id={`redact-reason-${seq}`} label="Reason (optional)">
            <input id={`redact-reason-${seq}`} className={inputClass} maxLength={500} value={reason} onChange={(e) => setReason(e.target.value)} />
          </Field>
          <label className="flex items-center gap-2 text-sm">
            <input type="checkbox" checked={reset} onChange={(e) => setReset(e.target.checked)} />
            Also reset the agent&apos;s session (it forgets this conversation)
          </label>
          {err ? (
            <p role="alert" className="text-sm text-danger-ink">
              {err}
            </p>
          ) : null}
          <Button type="submit" variant="danger" disabled={busy}>
            {busy ? "Redacting…" : "Redact"}
          </Button>
        </form>
      </DialogContent>
    </Dialog>
  );
}
