"use client";

import { useState } from "react";

import { FormError } from "@/components/auth/form-error";
import { Button } from "@/components/ui/button";
import { type RequestSubject, useAskRequest, useMyRequests, useWithdrawRequest } from "@/lib/api/app-setup-requests";
import type { WhoCanValue } from "@/lib/who-can";

/**
 * (P5-APIGAP-2, D-399, C-1047, D-407) **Ask an admin** for something only an organization admin can provide: a chat
 * platform or an AI key. The app journey's control and wording: once asked, "Asked the admins. You'll see it here when
 * it's done. · Undo". Admins are a role, never named. The ask is per organization and subject; a second person's ask
 * joins it, and nothing but the subject is sent (no free text).
 */
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
