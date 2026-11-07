"use client";

import { useState, type FormEvent } from "react";

import { Button } from "@/components/ui/button";
import { Field, inputClass } from "@/components/ui/field";
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
    <form onSubmit={(e) => void submit(e)} className="max-w-md space-y-4">
      <h2 className="text-lg font-medium text-red-800">Delete this workspace</h2>
      <p className="text-sm text-neutral-600">
        This permanently deletes the workspace and everything in it. Type{" "}
        <code className="font-mono">{slug}</code> to confirm.
      </p>
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
        <p role="alert" className="text-sm text-red-700">
          {errorMessage(error)}
        </p>
      ) : null}
      <Button type="submit" disabled={pending || confirm !== slug} className="bg-red-700 text-white">
        {pending ? "Deleting…" : "Delete workspace"}
      </Button>
    </form>
  );
}
