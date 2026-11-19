"use client";

import { useState } from "react";

import { FormError } from "@/components/auth/form-error";
import { Button } from "@/components/ui/button";
import { type RequestSubject, useAskRequest, useMyRequests, useWithdrawRequest } from "@/lib/api/app-setup-requests";
import type { WhoCanValue } from "@/lib/who-can";

const firstName = (full: string | null | undefined) => (full ? full.trim().split(/\s+/)[0]! : null);

/**
 * (P5-APIGAP-2, D-399, C-1047) **Ask <Name>** for something only an organization admin can provide: a chat platform or
 * an AI key. The app journey's control and wording: once asked, "Asked <Name> · Undo". The ask is per organization and
 * subject; a second person's ask joins it, and nothing but the subject is sent (no free text).
 */
export function AskAdmin({
  workspaceId,
  subject,
  connectorType,
  who,
}: {
  /** Any workspace of the organization the viewer belongs to: the ask is made from there. */
  workspaceId: string | undefined;
  subject: RequestSubject;
  connectorType?: string;
  who: WhoCanValue | null;
}) {
  const mine = useMyRequests(workspaceId);
  const ask = useAskRequest(workspaceId ?? "");
  const withdraw = useWithdrawRequest(workspaceId ?? "");
  const [local, setLocal] = useState<string | null | undefined>(undefined);
  if (!workspaceId) return null;
  const name = firstName(who?.people[0]?.name);
  const found = mine.data?.find((r) => r.subject === subject && (r.connector_type ?? null) === (connectorType ?? null));
  const requestId = local === undefined ? found?.id : local;
  if (requestId) {
    return (
      <span className="inline-flex flex-wrap items-center gap-2 text-sm text-ink-2" data-testid={`asked-${subject}`}>
        <span aria-hidden="true" className="size-2 shrink-0 rounded-full bg-accent" />
        <span>Asked {name ?? "an admin"}</span>
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
        aria-label={`Ask ${name ?? "an admin"} for ${subject === "ai_key" ? "an AI key" : "a chat platform"}`}>
        Ask {name ?? "an admin"}
      </Button>
      {ask.isError ? <FormError error={ask.error} /> : null}
    </span>
  );
}
