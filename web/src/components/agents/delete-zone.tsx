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
    <form onSubmit={(e) => void submit(e)} className="max-w-md space-y-3 rounded-md border border-red-200 p-4">
      <h2 className="text-lg font-medium text-red-800">Delete this agent</h2>
      <p className="text-sm text-neutral-600">
        Revokes all of its credentials. Type <code className="font-mono">{handle}</code> to confirm.
      </p>
      <Field id="del-agent" label="Agent handle">
        <input id="del-agent" className={inputClass} value={confirm} autoComplete="off" onChange={(e) => setConfirm(e.target.value)} />
      </Field>
      {del.error ? (
        <p role="alert" className="text-sm text-red-700">
          {errorMessage(del.error)}
        </p>
      ) : null}
      <Button type="submit" disabled={del.isPending || confirm !== handle} className="bg-red-700 text-white">
        {del.isPending ? "Deleting…" : "Delete agent"}
      </Button>
    </form>
  );
}
