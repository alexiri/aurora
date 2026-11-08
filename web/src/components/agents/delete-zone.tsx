"use client";

import { useState, type FormEvent } from "react";

import { Button } from "@/components/ui/button";
import { Field, inputClass } from "@/components/ui/field";
import { useDeleteAgent } from "@/lib/api/agents";
import { errorMessage } from "@/lib/api/errors";

export function DeleteZone({
  agentId,
  handle,
  wsId,
  onDeleted,
}: {
  agentId: string;
  handle: string;
  wsId: string;
  onDeleted: () => void;
}) {
  const del = useDeleteAgent(wsId, agentId);
  const [confirm, setConfirm] = useState("");

  async function submit(e: FormEvent) {
    e.preventDefault();
    try {
      await del.mutateAsync();
      onDeleted();
    } catch {
      // surfaced via del.error
    }
  }
  return (
    <form onSubmit={(e) => void submit(e)} className="max-w-md space-y-3 rounded-card border border-danger-ink/30 bg-surface p-4">
      <h2 className="text-lg font-semibold text-danger-ink">Delete this agent</h2>
      <p className="text-sm text-ink-2">
        Revokes all of its credentials. Type <code className="font-mono">{handle}</code> to confirm.
      </p>
      <Field id="del-agent" label="Agent handle">
        <input id="del-agent" className={inputClass} value={confirm} autoComplete="off" onChange={(e) => setConfirm(e.target.value)} />
      </Field>
      {del.error ? (
        <p role="alert" className="text-sm text-danger-ink">
          {errorMessage(del.error)}
        </p>
      ) : null}
      <Button type="submit" variant="danger" disabled={del.isPending || confirm !== handle}>
        {del.isPending ? "Deleting…" : "Delete agent"}
      </Button>
    </form>
  );
}
