"use client";

import { useState, type FormEvent } from "react";

import { Button } from "@/components/ui/button";
import { Field, inputClass } from "@/components/ui/field";
import { Notice } from "@/components/ui/notice";
import { DangerZone } from "@/components/ui/section";
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
    <DangerZone
      id="del-agent-zone"
      title="Delete this agent"
      description={
        <>
          Revokes all of its credentials. Type <code className="font-mono">{handle}</code> to confirm.
        </>
      }
    >
      <form onSubmit={(e) => void submit(e)} className="max-w-md space-y-3">
        <Field id="del-agent" label="Agent handle">
          <input id="del-agent" className={inputClass} value={confirm} autoComplete="off" onChange={(e) => setConfirm(e.target.value)} />
        </Field>
        {del.error ? (
          <Notice tone="danger" role="alert">
            {errorMessage(del.error)}
          </Notice>
        ) : null}
        <Button type="submit" variant="danger" disabled={del.isPending || confirm !== handle}>
          {del.isPending ? "Deleting…" : "Delete agent"}
        </Button>
      </form>
    </DangerZone>
  );
}
