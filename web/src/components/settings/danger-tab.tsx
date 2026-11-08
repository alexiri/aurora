"use client";

import { useState, type FormEvent } from "react";

import { Button } from "@/components/ui/button";
import { Field, inputClass } from "@/components/ui/field";
import { Form, FormActions } from "@/components/ui/form";
import { Notice } from "@/components/ui/notice";
import { DangerZone } from "@/components/ui/section";
import { deleteWorkspace } from "@/lib/api/account";
import { errorMessage } from "@/lib/api/errors";

export function DangerTab({
  wsId,
  slug,
  onDeleted,
}: {
  wsId: string;
  slug: string;
  onDeleted: () => void;
}) {
  const [confirm, setConfirm] = useState("");
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<unknown>(null);

  async function submit(e: FormEvent) {
    e.preventDefault();
    setPending(true);
    setError(null);
    try {
      await deleteWorkspace(wsId, confirm);
      onDeleted();
    } catch (err) {
      setError(err);
      setPending(false);
    }
  }

  return (
    <DangerZone
      id="delete-workspace"
      title="Delete this workspace"
      description={
        <>
          This permanently deletes the workspace and everything in it. Type <code className="font-mono">{slug}</code> to
          confirm.
        </>
      }
    >
      <Form onSubmit={(e) => void submit(e)} className="max-w-md">
        <Field id="del-ws" label="Workspace URL name">
          <input
            id="del-ws"
            className={inputClass}
            value={confirm}
            autoComplete="off"
            onChange={(e) => setConfirm(e.target.value)}
          />
        </Field>
        {error !== null ? (
          <Notice tone="danger" role="alert">
            {errorMessage(error)}
          </Notice>
        ) : null}
        <FormActions>
          <Button type="submit" variant="danger" size="lg" disabled={pending || confirm !== slug}>
            {pending ? "Deleting…" : "Delete workspace"}
          </Button>
        </FormActions>
      </Form>
    </DangerZone>
  );
}
