"use client";

import { useState, type FormEvent } from "react";

import { Button } from "@/components/ui/button";
import { Field, inputClass } from "@/components/ui/field";
import { Form, FormActions } from "@/components/ui/form";
import { ErrorNotice } from "@/components/ui/notice";
import { useAsyncAction } from "@/lib/use-async-action";

/**
 * The body of a delete `DangerZone`: type the URL name to enable the danger button; `remove` gets what was typed,
 * `onDeleted` runs on success. A failure shows under the field and re-enables the button.
 */
export function TypeToConfirmDelete({
  id,
  fieldLabel,
  slug,
  buttonLabel,
  size,
  remove,
  onDeleted,
}: {
  id: string;
  fieldLabel: string;
  slug: string;
  buttonLabel: string;
  size?: "lg";
  remove: (confirm: string) => Promise<unknown>;
  onDeleted: () => void;
}) {
  const [confirm, setConfirm] = useState("");
  const { pending, error, run } = useAsyncAction(() => remove(confirm), onDeleted);
  function submit(e: FormEvent) {
    e.preventDefault();
    if (confirm !== slug) return;
    void run();
  }
  return (
    <Form onSubmit={submit} className="max-w-md">
      <Field id={id} label={fieldLabel}>
        <input id={id} className={inputClass} value={confirm} autoComplete="off" onChange={(e) => setConfirm(e.target.value)} />
      </Field>
      <ErrorNotice error={error} />
      <FormActions>
        <Button type="submit" variant="danger" size={size} disabled={pending || confirm !== slug}>
          {pending ? "Deleting…" : buttonLabel}
        </Button>
      </FormActions>
    </Form>
  );
}
