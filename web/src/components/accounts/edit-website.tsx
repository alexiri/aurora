"use client";

import { useState, type FormEvent } from "react";

import { RecipeFieldsInput } from "@/components/credentials/generic-form";
import { ErrorState, Loading } from "@/components/states";
import { Button } from "@/components/ui/button";
import { Dialog, DialogClose, DialogContent } from "@/components/ui/dialog";
import { Field, inputClass } from "@/components/ui/field";
import { Notice } from "@/components/ui/notice";
import { errorMessage, fieldErrors } from "@/lib/api/errors";
import { buildRecipe, recipeToFields, useSecrets, usePatchSecret, type RecipeKind, type Secret, type SecretScope } from "@/lib/api/secrets";

function Form({ scope, secret, onClose }: { scope: SecretScope; secret: Secret; onClose: () => void }) {
  const patch = usePatchSecret(scope, secret.id);
  const [host, setHost] = useState(secret.host_pattern);
  const [path, setPath] = useState(secret.path_pattern ?? "");
  const [kind, setKind] = useState<RecipeKind>(secret.injection.kind);
  const [fields, setFields] = useState(recipeToFields(secret.injection));
  const fe = patch.error ? fieldErrors(patch.error) : {};

  async function submit(e: FormEvent) {
    e.preventDefault();
    try {
      await patch.mutateAsync({ host_pattern: host.trim(), path_pattern: path.trim() || null, injection: buildRecipe(kind, fields) });
      onClose();
    } catch {
      // The error is shown below; what was typed stays.
    }
  }

  const id = `ew-${secret.id}`;
  return (
    <form onSubmit={(e) => void submit(e)} className="space-y-4" aria-label={`Change website of ${secret.name}`} noValidate>
      <Field id={`${id}-site`} label="Website" hint="For example api.example.com" error={fe.host_pattern}>
        <input id={`${id}-site`} className={inputClass} value={host} onChange={(e) => setHost(e.target.value)} />
      </Field>
      <details className="rounded-control border border-line p-3">
        <summary className="min-h-9 cursor-pointer text-sm font-semibold text-ink">Advanced: path, other ways to send the token</summary>
        <div className="mt-3 space-y-4">
          <Field id={`${id}-path`} label="Path (optional)" error={fe.path_pattern}>
            <input id={`${id}-path`} className={inputClass} value={path} onChange={(e) => setPath(e.target.value)} />
          </Field>
          <RecipeFieldsInput idp={id} kind={kind} fields={fields} onKind={setKind} onFields={setFields} />
        </div>
      </details>
      {patch.error ? <Notice tone="danger" role="alert">{errorMessage(patch.error)}</Notice> : null}
      <div className="flex justify-end gap-2">
        <DialogClose asChild>
          <Button variant="outline" type="button">Cancel</Button>
        </DialogClose>
        <Button type="submit" disabled={!host.trim() || patch.isPending}>{patch.isPending ? "Saving…" : "Save"}</Button>
      </div>
    </form>
  );
}

/**
 * **Change website** (the row menu of a website account, `14 §1`): the website the token is sent to, with the path and the
 * way the token is sent under Advanced (`08 §5`). The accounts list does not carry these (P5-UX-4), so the website's own
 * record is read when the dialog opens. A name change is Rename's.
 */
export function EditWebsite({ scope, accountId, name, open, onOpenChange }: { scope: SecretScope; accountId: string; name: string; open: boolean; onOpenChange: (o: boolean) => void }) {
  const secrets = useSecrets(scope, open);
  const secret = secrets.data?.items.find((s) => s.id === accountId);
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent title={`Change website of ${name}`}>
        {secrets.isPending ? (
          <Loading />
        ) : secrets.isError ? (
          <ErrorState error={secrets.error} onRetry={() => void secrets.refetch()} />
        ) : secret ? (
          <Form scope={scope} secret={secret} onClose={() => onOpenChange(false)} />
        ) : (
          <Notice tone="danger" role="alert">This website could not be found. It may have been removed.</Notice>
        )}
      </DialogContent>
    </Dialog>
  );
}
