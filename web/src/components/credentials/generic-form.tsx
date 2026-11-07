"use client";

import { useState, type FormEvent } from "react";

import { Button } from "@/components/ui/button";
import { Field, inputClass } from "@/components/ui/field";
import { errorMessage, fieldErrors } from "@/lib/api/errors";
import {
  buildGenericCreate,
  EMPTY_RECIPE_FIELDS,
  RECIPE_KINDS,
  referencesAvailable,
  useCreateSecret,
  useSecretStores,
  type GenericFormState,
  type RecipeFields,
  type RecipeKind,
  type SecretScope,
} from "@/lib/api/secrets";

import { ValueInput } from "./value-input";

/** Per-kind recipe inputs; shared by create and metadata edit. */
export function RecipeFieldsInput({
  idp,
  kind,
  fields,
  onKind,
  onFields,
}: {
  idp: string;
  kind: RecipeKind;
  fields: RecipeFields;
  onKind: (k: RecipeKind) => void;
  onFields: (f: RecipeFields) => void;
}) {
  const set = (k: keyof RecipeFields) => (e: { target: { value: string } }) =>
    onFields({ ...fields, [k]: e.target.value });
  const text = (k: keyof RecipeFields, label: string, hint?: string) => (
    <Field id={`${idp}-${k}`} label={label} hint={hint}>
      <input id={`${idp}-${k}`} className={inputClass} value={fields[k]} onChange={set(k)} />
    </Field>
  );
  return (
    <>
      <Field id={`${idp}-kind`} label="Injection method">
        <select id={`${idp}-kind`} className={inputClass} value={kind} onChange={(e) => onKind(e.target.value as RecipeKind)}>
          {RECIPE_KINDS.map((k) => (
            <option key={k.kind} value={k.kind}>
              {k.label}
            </option>
          ))}
        </select>
      </Field>
      {kind === "header" || kind === "query" ? (
        <>
          {text("name", kind === "header" ? "Header name" : "Parameter name")}
          {text("format", "Format (optional)", "Use {value} where the secret goes.")}
        </>
      ) : null}
      {kind === "basic" ? text("username", "Username") : null}
      {kind === "path_template" ? text("prefix", "Path prefix") : null}
      {kind === "path_regex" ? (
        <>
          {text("pattern", "Pattern")}
          {text("replacement", "Replacement")}
        </>
      ) : null}
    </>
  );
}

const initial = (): GenericFormState => ({
  name: "", host: "", path: "", kind: "bearer", fields: EMPTY_RECIPE_FIELDS, source: "value", value: "",
  ref: { store_id: "", path: "", field: "" },
});

export function GenericSecretForm({ scope, onDone }: { scope: SecretScope; onDone?: () => void }) {
  const create = useCreateSecret(scope);
  const stores = useSecretStores();
  const refStores = (stores.data?.items ?? []).filter((s) => s.capabilities.references);
  const canRef = referencesAvailable(stores.data?.items);
  const [s, setS] = useState<GenericFormState>(initial);
  const [clientErr, setClientErr] = useState("");
  const fe = create.error ? fieldErrors(create.error) : {};

  async function submit(e: FormEvent) {
    e.preventDefault();
    if (!s.name.trim() || !s.host.trim()) return setClientErr("Name and host pattern are required.");
    if (s.source === "value" && !s.value) return setClientErr("Enter a value.");
    if (s.source === "reference" && (!s.ref.store_id || !s.ref.path.trim()))
      return setClientErr("Choose a store and enter a path.");
    setClientErr("");
    try {
      await create.mutateAsync(buildGenericCreate(s));
      setS(initial());
      onDone?.();
    } catch {
      setS((p) => ({ ...p, value: "" }));
    }
  }

  return (
    <form onSubmit={(e) => void submit(e)} noValidate className="max-w-xl space-y-3" aria-label="New secret">
      <Field id="g-name" label="Name" error={fe.name}>
        <input id="g-name" className={inputClass} value={s.name} onChange={(e) => setS({ ...s, name: e.target.value })} />
      </Field>
      <Field id="g-host" label="Host pattern" hint="For example api.example.com or *.example.com" error={fe.host_pattern}>
        <input id="g-host" className={inputClass} value={s.host} onChange={(e) => setS({ ...s, host: e.target.value })} />
      </Field>
      <Field id="g-path" label="Path pattern (optional)" error={fe.path_pattern}>
        <input id="g-path" className={inputClass} value={s.path} onChange={(e) => setS({ ...s, path: e.target.value })} />
      </Field>
      <RecipeFieldsInput
        idp="g"
        kind={s.kind}
        fields={s.fields}
        onKind={(kind) => setS({ ...s, kind })}
        onFields={(fields) => setS({ ...s, fields })}
      />
      {canRef ? (
        <Field id="g-source" label="Value source">
          <select
            id="g-source"
            className={inputClass}
            value={s.source}
            onChange={(e) => setS({ ...s, source: e.target.value as "value" | "reference" })}
          >
            <option value="value">Store the value in Aurora</option>
            <option value="reference">Reference an external store</option>
          </select>
        </Field>
      ) : null}
      {s.source === "reference" && canRef ? (
        <>
          <Field id="g-store" label="Store">
            <select
              id="g-store"
              className={inputClass}
              value={s.ref.store_id}
              onChange={(e) => setS({ ...s, ref: { ...s.ref, store_id: e.target.value } })}
            >
              <option value="">Select a store</option>
              {refStores.map((st) => (
                <option key={st.id} value={st.id}>
                  {st.id} ({st.kind})
                </option>
              ))}
            </select>
          </Field>
          <Field id="g-refpath" label="Path">
            <input id="g-refpath" className={inputClass} value={s.ref.path} onChange={(e) => setS({ ...s, ref: { ...s.ref, path: e.target.value } })} />
          </Field>
          <Field id="g-reffield" label="Field (optional)">
            <input id="g-reffield" className={inputClass} value={s.ref.field} onChange={(e) => setS({ ...s, ref: { ...s.ref, field: e.target.value } })} />
          </Field>
        </>
      ) : (
        <ValueInput id="g-value" value={s.value} onChange={(value) => setS({ ...s, value })} error={fe.value} />
      )}
      {clientErr || create.error ? (
        <p role="alert" className="text-sm text-red-700">
          {clientErr || errorMessage(create.error)}
        </p>
      ) : null}
      <Button type="submit" disabled={create.isPending}>
        {create.isPending ? "Saving…" : "Add secret"}
      </Button>
    </form>
  );
}
