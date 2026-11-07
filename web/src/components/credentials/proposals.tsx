"use client";

import { useState, type FormEvent } from "react";

import { Button } from "@/components/ui/button";
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
    <li className="space-y-2 rounded-md border border-neutral-200 p-3">
      <p>
        <span className="font-medium">{secret.name}</span>{" "}
        <span className="text-xs text-neutral-500">
          {secret.injection.kind} · {secret.host_pattern}
          {secret.path_pattern ?? ""}
        </span>
      </p>
      <p className="text-xs text-neutral-600">An agent proposed this secret and is waiting for a value.</p>
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
      {err ? <p role="alert" className="text-sm text-red-700">{errorMessage(err)}</p> : null}
    </li>
  );
}

export function ProposalsList({ scope, items, canOwn }: { scope: SecretScope; items: Secret[]; canOwn: boolean }) {
  const proposed = items.filter((s) => s.status === "proposed");
  if (proposed.length === 0) return <p className="text-sm text-neutral-600">No proposals are waiting for a value.</p>;
  return (
    <ul className="space-y-2">
      {proposed.map((s) => (
        <Proposal key={s.id} scope={scope} secret={s} canOwn={canOwn} />
      ))}
    </ul>
  );
}
