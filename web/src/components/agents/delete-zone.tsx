"use client";

import { useState, type FormEvent } from "react";

import { FormDialog } from "@/components/ui/confirm-dialog";
import { Field, inputClass } from "@/components/ui/field";
import { ErrorNotice } from "@/components/ui/notice";
import { useDeleteAgent } from "@/lib/api/agents";

/**
 * Delete agent (D-200: opened from the page's Agent actions menu; the Delete zone under Instructions is gone). The
 * same typed confirmation as before: the agent's handle. Only workspace owners are offered it; the API checks again.
 */
export function DeleteAgentDialog({
  agentId,
  handle,
  wsId,
  open,
  onOpenChange,
  onDeleted,
}: {
  agentId: string;
  handle: string;
  wsId: string;
  open: boolean;
  onOpenChange: (open: boolean) => void;
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
    <FormDialog
      open={open}
      onOpenChange={(o) => {
        if (!o) setConfirm("");
        onOpenChange(o);
      }}
      title="Delete this agent"
      description="Revokes all of its credentials. This cannot be undone."
      submitLabel={del.isPending ? "Deleting…" : "Delete agent"}
      danger
      busy={del.isPending}
      submitDisabled={confirm !== handle}
      onSubmit={(e) => void submit(e)}
    >
      <Field id="del-agent" label="Agent handle" hint={`Type ${handle} to confirm.`}>
        <input id="del-agent" className={inputClass} value={confirm} autoComplete="off" onChange={(e) => setConfirm(e.target.value)} />
      </Field>
      <ErrorNotice error={del.error} />
    </FormDialog>
  );
}
