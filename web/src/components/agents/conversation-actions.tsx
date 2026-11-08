"use client";

import { useQueryClient } from "@tanstack/react-query";
import { useState, type FormEvent } from "react";

import { Button } from "@/components/ui/button";
import { FormDialog } from "@/components/ui/confirm-dialog";
import { Field, inputClass } from "@/components/ui/field";
import { CheckboxField } from "@/components/ui/form";
import { MoreIcon } from "@/components/ui/icons";
import { Menu, MenuContent, MenuItem, MenuSeparator, MenuTrigger } from "@/components/ui/menu";
import { Notice } from "@/components/ui/notice";
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

/**
 * The thread header's "Conversation actions" menu (RFE-010): archive/unarchive, export, and delete with a
 * typed confirmation. The dialogs are unchanged; only the triggers moved into a menu (D-102).
 */
export function ConversationActions({
  agentId,
  convId,
  archived,
  onGone,
  onError,
}: {
  agentId: string;
  convId: string;
  archived: boolean;
  /** The conversation is no longer in this list (archived or deleted): the parent picks another. */
  onGone: () => void;
  /** A failure from a menu action (shown by the parent as an alert); `null` clears it. */
  onError?: (message: string | null) => void;
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

  async function run(fn: () => Promise<unknown>, after?: () => void, inDialog = false) {
    setBusy(true);
    setErr(null);
    onError?.(null);
    try {
      await fn();
      refresh();
      after?.();
    } catch (e) {
      const msg = lifecycleError(e);
      if (inDialog) setErr(msg);
      else onError?.(msg);
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
      true,
    );
  }

  return (
    <>
      {/* Non-modal: a modal menu aria-hides the rest of the page while its focusable controls stay reachable (axe: aria-hidden-focus). */}
      <Menu modal={false}>
        <MenuTrigger asChild>
          <Button variant="ghost" size="sm" aria-label="Conversation actions" disabled={busy} className="size-11 min-h-11 px-0 md:size-9 md:min-h-9">
            <MoreIcon width={20} height={20} />
          </Button>
        </MenuTrigger>
        <MenuContent align="end">
          <MenuItem
            disabled={busy}
            onSelect={() => void run(() => (archived ? chatApi.unarchive(convId) : chatApi.archive(convId)), archived ? undefined : onGone)}>
            {archived ? "Unarchive" : "Archive"}
          </MenuItem>
          <MenuItem asChild>
            <a href={exportUrl(convId, "json")} download>
              Export JSON
            </a>
          </MenuItem>
          <MenuItem asChild>
            <a href={exportUrl(convId, "zip")} download>
              Export with files (zip)
            </a>
          </MenuItem>
          <MenuSeparator />
          <MenuItem className="text-danger-ink data-[highlighted]:bg-danger-soft" onSelect={() => setOpen(true)}>
            Delete conversation…
          </MenuItem>
        </MenuContent>
      </Menu>
      <FormDialog
        open={open}
        onOpenChange={(o) => {
          setOpen(o);
          setTyped("");
          setErr(null);
        }}
        title="Delete this conversation?"
        description="This permanently deletes the messages, attachments and the agent's stored session for this conversation. It cannot be undone."
        submitLabel={busy ? "Deleting…" : "Delete conversation"}
        danger
        busy={busy}
        submitDisabled={typed.trim().toLowerCase() !== CONFIRM_WORD}
        onSubmit={confirmDelete}>
        <p className="text-sm text-ink-2">
          Messages already posted to a chat platform, memories or skills the agent derived from this conversation, and exports you already downloaded are
          not deleted.
        </p>
        <Field id="delete-confirm" label={`Type "${CONFIRM_WORD}" to confirm`}>
          <input id="delete-confirm" className={inputClass} autoComplete="off" value={typed} onChange={(e) => setTyped(e.target.value)} />
        </Field>
        {err ? (
          <Notice tone="danger" role="alert">
            {err}
          </Notice>
        ) : null}
      </FormDialog>
    </>
  );
}

/** Redact dialog for one message: replaces the content in place (irreversible); optionally resets the session. Opened from the message toolbar. */
export function RedactDialog({ convId, seq, open, onOpenChange }: { convId: string; seq: number; open: boolean; onOpenChange: (open: boolean) => void }) {
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
      onOpenChange(false);
    } catch (e2) {
      setErr(lifecycleError(e2));
    } finally {
      setBusy(false);
    }
  }

  return (
    <FormDialog
      open={open}
      onOpenChange={(o) => {
        onOpenChange(o);
        setErr(null);
      }}
      title="Redact this message?"
      description="The content is replaced for everyone and cannot be restored."
      submitLabel={busy ? "Redacting…" : "Redact"}
      danger
      busy={busy}
      onSubmit={(e) => void submit(e)}>
      <Notice tone="pending" role="note">
        This is irreversible. The agent may still remember what it read; tick the box below to start it with a fresh session.
      </Notice>
      <Field id={`redact-reason-${seq}`} label="Reason (optional)">
        <input id={`redact-reason-${seq}`} className={inputClass} maxLength={500} value={reason} onChange={(e) => setReason(e.target.value)} />
      </Field>
      <CheckboxField id={`redact-reset-${seq}`} label="Also reset the agent's session (it forgets this conversation)" checked={reset} onChange={setReset} />
      {err ? (
        <Notice tone="danger" role="alert">
          {err}
        </Notice>
      ) : null}
    </FormDialog>
  );
}
