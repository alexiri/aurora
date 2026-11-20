"use client";

import { useState } from "react";

import { Field, inputClass } from "@/components/ui/field";
import { FormDialog } from "@/components/ui/confirm-dialog";

/** The most a decline message may hold (C-1081). */
export const DECLINE_MESSAGE_MAX = 300;

const first = (name: string) => name.trim().split(/\s+/)[0] ?? name;

/** "Carol", "Bob and Carol", "Bob, Carol and 2 others": first names for the message field's label. */
function firstNames(names: string[]): string {
  if (names.length <= 1) return first(names[0] ?? "the asker");
  if (names.length === 2) return `${first(names[0]!)} and ${first(names[1]!)}`;
  const others = names.length - 2;
  return `${first(names[0]!)}, ${first(names[1]!)} and ${others} ${others === 1 ? "other" : "others"}`;
}

/**
 * (P5-REFINE-18 #13, setup-8, C-1081, D-407) One Decline dialog for every request card: the Overview's Needs you, the
 * organization's Apps and Administration. It says who is told, takes an optional message the askers read with the notice
 * (at most 300 characters) and never names the decider's login. `noun` is what was asked for ("a chat platform", "Jira").
 * The caller owns `open` and closes it when the decline went through.
 */
export function DeclineRequestDialog({
  open,
  onOpenChange,
  names,
  noun,
  busy,
  error,
  onDecline,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** The askers' display names (SEC-135: names only). */
  names: string[];
  noun: string;
  busy?: boolean;
  error?: unknown;
  onDecline: (message: string | undefined) => void;
}) {
  const [message, setMessage] = useState("");
  const who = names.length === 0 ? "Someone" : names.length === 1 ? names[0]! : names.length === 2 ? `${names[0]} and ${names[1]}` : `${names.length} people`;
  const label = firstNames(names);
  return (
    <FormDialog
      open={open}
      onOpenChange={(o) => {
        if (!o) setMessage("");
        onOpenChange(o);
      }}
      title={`Decline ${label}’s request?`}
      description={`${who} asked for ${noun}. ${names.length === 1 ? "They" : "Each of them"} will be told you declined.`}
      submitLabel="Decline"
      busy={busy}
      error={error}
      onSubmit={(e) => {
        e.preventDefault();
        const text = message.trim();
        onDecline(text === "" ? undefined : text);
      }}
    >
      <Field id="decline-message" label={`Message for ${label} (optional)`} hint={`${label} ${names.length > 1 ? "see" : "sees"} this with the notice.`}>
        <textarea
          id="decline-message"
          rows={3}
          maxLength={DECLINE_MESSAGE_MAX}
          className={inputClass}
          value={message}
          onChange={(e) => setMessage(e.target.value)}
        />
      </Field>
    </FormDialog>
  );
}
