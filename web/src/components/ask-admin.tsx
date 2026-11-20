"use client";

import { useState } from "react";

import { FormError } from "@/components/auth/form-error";
import { Button } from "@/components/ui/button";
import { inputClass } from "@/components/ui/field";
import { ShieldIcon } from "@/components/ui/icons";
import { type RequestSubject, useAskRequest, useMyRequests, useWithdrawRequest } from "@/lib/api/app-setup-requests";
import type { WhoCanValue } from "@/lib/who-can";

/**
 * (P5-APIGAP-2, D-399, C-1047, D-407) **Ask an admin** for something only an organization admin can provide: a chat
 * platform or an AI key. The app journey's control and wording: once asked, "Asked the admins. You'll see it here when
 * it's done. · Undo". Admins are a role, never named. The ask is per organization and subject; a second person's ask
 * joins it, and nothing but the subject is sent (no free text).
 */
/** The reason's longest length (C-1081). */
export const REASON_MAX = 500;

/** "Slack, Teams or Mattermost" from a list of names. */
export function nameList(names: string[]): string {
  if (names.length <= 1) return names.join("");
  return `${names.slice(0, -1).join(", ")} or ${names[names.length - 1]}`;
}

/**
 * (P5-REFINE-14, channels-3, D-417) **Ask an admin** with an optional **Why do you need it?** and **Send request**. The ask
 * goes to every admin who can do it, so no one's name is in the control; `platforms` ("Slack, Teams or Mattermost") says what
 * is asked for. After it: "Asked an admin · Undo".
 */
export function AskAdminForm({
  workspaceId,
  subject,
  connectorType,
  platforms,
  placeholder = "So Dani can answer in #ops",
}: {
  workspaceId: string | undefined;
  subject: RequestSubject;
  connectorType?: string;
  platforms?: string;
  placeholder?: string;
}) {
  const mine = useMyRequests(workspaceId);
  const ask = useAskRequest(workspaceId ?? "");
  const withdraw = useWithdrawRequest(workspaceId ?? "");
  const [local, setLocal] = useState<string | null | undefined>(undefined);
  const [reason, setReason] = useState("");
  if (!workspaceId) return null;
  const found = mine.data?.find((r) => r.subject === subject && (r.connector_type ?? null) === (connectorType ?? null));
  const requestId = local === undefined ? found?.id : local;
  if (requestId) {
    return (
      <span className="inline-flex flex-wrap items-center gap-2 text-sm text-ink-2" data-testid={`asked-${subject}`}>
        <span aria-hidden="true" className="size-2 shrink-0 rounded-full bg-accent" />
        <span>Asked an admin</span>
        <span aria-hidden="true">·</span>
        <Button
          type="button"
          variant="ghost"
          size="sm"
          disabled={withdraw.isPending}
          onClick={() => withdraw.mutate(requestId, { onSuccess: () => setLocal(null) })}>
          Undo
        </Button>
      </span>
    );
  }
  const what = subject === "ai_key" ? "an AI key" : "a chat platform";
  return (
    <form
      className="w-full max-w-xl space-y-3"
      onSubmit={(e) => {
        e.preventDefault();
        const text = reason.trim();
        ask.mutate({ subject, connectorType, ...(text ? { reason: text } : {}) }, { onSuccess: (r) => setLocal(r.id) });
      }}>
      <p className="inline-flex items-center gap-2 rounded-control bg-subtle px-3 py-2 text-sm text-ink-2">
        <ShieldIcon aria-hidden="true" className="size-4 shrink-0" />
        <span>
          Ask <strong className="font-semibold text-ink">an admin</strong>
          {platforms ? ` to connect ${platforms}` : ` for ${what}`}
        </span>
      </p>
      <div className="space-y-1.5">
        <label htmlFor={`ask-reason-${subject}`} className="block text-sm font-medium text-ink">
          Why do you need it? <span className="font-normal text-muted">(optional)</span>
        </label>
        <input
          id={`ask-reason-${subject}`}
          className={inputClass}
          value={reason}
          maxLength={REASON_MAX}
          placeholder={placeholder}
          onChange={(e) => setReason(e.target.value)}
        />
      </div>
      <Button type="submit" size="sm" disabled={ask.isPending}>
        Send request
      </Button>
      {ask.isError ? <FormError error={ask.error} /> : null}
    </form>
  );
}

export function AskAdmin({
  workspaceId,
  subject,
  connectorType,
}: {
  /** Any workspace of the organization the viewer belongs to: the ask is made from there. */
  workspaceId: string | undefined;
  subject: RequestSubject;
  connectorType?: string;
  /** Kept for callers; admins are never named (D-407), so it is not read. */
  who?: WhoCanValue | null;
}) {
  const mine = useMyRequests(workspaceId);
  const ask = useAskRequest(workspaceId ?? "");
  const withdraw = useWithdrawRequest(workspaceId ?? "");
  const [local, setLocal] = useState<string | null | undefined>(undefined);
  if (!workspaceId) return null;
  const found = mine.data?.find((r) => r.subject === subject && (r.connector_type ?? null) === (connectorType ?? null));
  const requestId = local === undefined ? found?.id : local;
  if (requestId) {
    return (
      <span className="inline-flex flex-wrap items-center gap-2 text-sm text-ink-2" data-testid={`asked-${subject}`}>
        <span aria-hidden="true" className="size-2 shrink-0 rounded-full bg-accent" />
        <span>Asked the admins. You&apos;ll see it here when it&apos;s done.</span>
        <span aria-hidden="true">·</span>
        <Button
          type="button"
          variant="ghost"
          size="sm"
          disabled={withdraw.isPending}
          onClick={() => withdraw.mutate(requestId, { onSuccess: () => setLocal(null) })}>
          Undo
        </Button>
      </span>
    );
  }
  return (
    <span className="inline-flex flex-col items-start gap-1">
      <Button
        type="button"
        variant="outline"
        size="sm"
        disabled={ask.isPending}
        onClick={() => ask.mutate({ subject, connectorType }, { onSuccess: (r) => setLocal(r.id) })}
        aria-label={`Ask an admin for ${subject === "ai_key" ? "an AI key" : "a chat platform"}`}>
        Ask an admin
      </Button>
      {ask.isError ? <FormError error={ask.error} /> : null}
    </span>
  );
}
