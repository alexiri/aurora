"use client";

import { useState, type FormEvent } from "react";

import { EmptyState } from "@/components/states";
import { Button } from "@/components/ui/button";
import { Notice } from "@/components/ui/notice";
import { errorMessage } from "@/lib/api/errors";
import { useAcceptSecret, useRejectSecret, type Secret, type SecretScope } from "@/lib/api/secrets";

import { ValueInput } from "./value-input";

function Proposal({ scope, secret, canOwn }: { scope: SecretScope; secret: Secret; canOwn: boolean }) {
  const accept = useAcceptSecret(scope, secret.id);
  const reject = useRejectSecret(scope, secret.id);
  const [value, setValue] = useState("");
  async function submit(e: FormEvent) {
    e.preventDefault();
    if (!value) return;
    try {
      await accept.mutateAsync(value);
    } catch {
      // error shown below
    } finally {
      setValue("");
    }
  }
  const err = accept.error ?? reject.error;
  return (
    <li className="space-y-2 rounded-card border border-pending-line bg-pending-panel p-4">
      <p>
        <span className="font-medium text-ink">{secret.name}</span>{" "}
        <span className="break-all font-mono text-xs text-muted">
          {secret.injection.kind} · {secret.host_pattern}
          {secret.path_pattern ?? ""}
        </span>
      </p>
      <p className="text-xs text-pending-ink">An agent proposed this secret and is waiting for a value.</p>
      {canOwn ? (
        <form onSubmit={(e) => void submit(e)} aria-label={`Accept ${secret.name}`} className="space-y-2">
          <ValueInput id={`ap-${secret.id}`} value={value} onChange={setValue} />
          <div className="flex gap-2">
            <Button type="submit" size="sm" disabled={accept.isPending}>Accept with value</Button>
            <Button type="button" size="sm" variant="outline" disabled={reject.isPending} onClick={() => reject.mutate()}>
              Reject
            </Button>
          </div>
        </form>
      ) : null}
      {err ? <Notice tone="danger" role="alert">{errorMessage(err)}</Notice> : null}
    </li>
  );
}

export function ProposalsList({ scope, items, canOwn }: { scope: SecretScope; items: Secret[]; canOwn: boolean }) {
  const proposed = items.filter((s) => s.status === "proposed");
  if (proposed.length === 0) return <EmptyState title="No proposals are waiting for a value" />;
  return (
    <ul className="space-y-3">
      {proposed.map((s) => (
        <Proposal key={s.id} scope={scope} secret={s} canOwn={canOwn} />
      ))}
    </ul>
  );
}
