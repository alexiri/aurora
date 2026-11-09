"use client";

import { useState, type FormEvent } from "react";

import { Button } from "@/components/ui/button";
import { Field, inputClass } from "@/components/ui/field";
import { Notice } from "@/components/ui/notice";
import { errorMessage } from "@/lib/api/errors";
import {
  buildRecipe,
  recipeToFields,
  useDeleteSecret,
  usePatchSecret,
  useSetSecretValue,
  useVerifySecret,
  type RecipeKind,
  type Secret,
  type SecretScope,
} from "@/lib/api/secrets";

import { RecipeFieldsInput } from "./generic-form";
import { AccessDialog } from "./give-access";
import { OriginView } from "./origin-view";
import { ValueInput } from "./value-input";

function ReplaceValue({ scope, secret, onClose }: { scope: SecretScope; secret: Secret; onClose: () => void }) {
  const set = useSetSecretValue(scope, secret.id);
  const [value, setValue] = useState("");
  async function submit(e: FormEvent) {
    e.preventDefault();
    if (!value) return;
    try {
      await set.mutateAsync({ value });
      onClose();
    } catch {
      // error shown below
    } finally {
      setValue("");
    }
  }
  return (
    <form onSubmit={(e) => void submit(e)} className="space-y-2" aria-label={`Replace value of ${secret.name}`}>
      <ValueInput id={`rv-${secret.id}`} label="New value" value={value} onChange={setValue} />
      {set.error ? <Notice tone="danger" role="alert">{errorMessage(set.error)}</Notice> : null}
      <div className="flex flex-wrap gap-2">
        <Button type="submit" size="sm" disabled={set.isPending}>Replace</Button>
        <Button type="button" size="sm" variant="outline" onClick={onClose}>Cancel</Button>
      </div>
    </form>
  );
}

function EditMeta({ scope, secret, onClose }: { scope: SecretScope; secret: Secret; onClose: () => void }) {
  const patch = usePatchSecret(scope, secret.id);
  const generic = secret.kind === "generic";
  const [name, setName] = useState(secret.name);
  const [host, setHost] = useState(secret.host_pattern);
  const [path, setPath] = useState(secret.path_pattern ?? "");
  const [kind, setKind] = useState<RecipeKind>(secret.injection.kind);
  const [fields, setFields] = useState(recipeToFields(secret.injection));
  async function submit(e: FormEvent) {
    e.preventDefault();
    try {
      await patch.mutateAsync(
        generic
          ? { name: name.trim(), host_pattern: host.trim(), path_pattern: path.trim() || null, injection: buildRecipe(kind, fields) }
          : { name: name.trim() },
      );
      onClose();
    } catch {
      // error shown below
    }
  }
  const id = `e-${secret.id}`;
  return (
    <form onSubmit={(e) => void submit(e)} className="space-y-2" aria-label={`Edit ${secret.name}`}>
      <Field id={`${id}-name`} label="Name">
        <input id={`${id}-name`} className={inputClass} value={name} onChange={(e) => setName(e.target.value)} />
      </Field>
      {generic ? (
        <>
          <Field id={`${id}-host`} label="Host pattern">
            <input id={`${id}-host`} className={inputClass} value={host} onChange={(e) => setHost(e.target.value)} />
          </Field>
          <Field id={`${id}-path`} label="Path pattern (optional)">
            <input id={`${id}-path`} className={inputClass} value={path} onChange={(e) => setPath(e.target.value)} />
          </Field>
          <RecipeFieldsInput idp={id} kind={kind} fields={fields} onKind={setKind} onFields={setFields} />
        </>
      ) : null}
      {patch.error ? <Notice tone="danger" role="alert">{errorMessage(patch.error)}</Notice> : null}
      <div className="flex flex-wrap gap-2">
        <Button type="submit" size="sm" disabled={patch.isPending}>Save</Button>
        <Button type="button" size="sm" variant="outline" onClick={onClose}>Cancel</Button>
      </div>
    </form>
  );
}

export function SecretRow({ scope, orgId, secret, canOwn }: { scope: SecretScope; orgId: string; secret: Secret; canOwn: boolean }) {
  const [mode, setMode] = useState<"none" | "replace" | "edit" | "delete">("none");
  const [accessOpen, setAccessOpen] = useState(false);
  const del = useDeleteSecret(scope, secret.id);
  const verify = useVerifySecret(scope, secret.id);
  const close = () => setMode("none");
  return (
    <li className="space-y-3 rounded-card border border-line bg-surface p-4 shadow-card">
      <div className="flex flex-wrap items-center justify-between gap-x-3 gap-y-2">
        <div className="min-w-0">
          <span className="font-medium text-ink">{secret.name}</span>{" "}
          <span className="break-all font-mono text-xs text-muted">
            {secret.kind === "llm" ? `LLM · ${secret.llm?.provider ?? ""}` : secret.injection.kind} · {secret.host_pattern}
            {secret.path_pattern ? secret.path_pattern : ""} · {secret.value_source} · v{secret.value_version}
          </span>
        </div>
        {canOwn ? (
          <div className="flex flex-wrap gap-2">
            {secret.kind === "llm" ? (
              <Button size="sm" variant="outline" disabled={verify.isPending} onClick={() => verify.mutate()}>
                Re-verify
              </Button>
            ) : null}
            <Button size="sm" variant="outline" aria-label={`Access to ${secret.name}`} onClick={() => setAccessOpen(true)}>
              Access
            </Button>
            <Button size="sm" variant="outline" onClick={() => setMode("replace")}>Replace value</Button>
            <Button size="sm" variant="outline" onClick={() => setMode("edit")}>Edit</Button>
            <Button size="sm" variant="danger" onClick={() => setMode("delete")}>Delete</Button>
          </div>
        ) : null}
      </div>
      {secret.kind === "llm" ? <OriginView origin={secret.origin} /> : null}
      {verify.error ? <Notice tone="danger" role="alert">{errorMessage(verify.error)}</Notice> : null}
      {canOwn ? (
        <AccessDialog
          subject={{ target: { kind: "secret", id: secret.id }, scope, orgId }}
          title={secret.name}
          open={accessOpen}
          onOpenChange={setAccessOpen}
        />
      ) : null}
      {mode === "replace" ? <ReplaceValue scope={scope} secret={secret} onClose={close} /> : null}
      {mode === "edit" ? <EditMeta scope={scope} secret={secret} onClose={close} /> : null}
      {mode === "delete" ? (
        <Notice tone="danger" role="alertdialog" aria-label={`Delete ${secret.name}`} className="space-y-2">
          <p>Delete {secret.name}? Agents will lose access to it.</p>
          {del.error ? <p role="alert">{errorMessage(del.error)}</p> : null}
          <div className="flex gap-2">
            <Button size="sm" variant="danger" disabled={del.isPending} onClick={() => del.mutate()}>Confirm delete</Button>
            <Button size="sm" variant="outline" onClick={close}>Cancel</Button>
          </div>
        </Notice>
      ) : null}
    </li>
  );
}
