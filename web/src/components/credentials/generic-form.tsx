"use client";

import { Field, inputClass } from "@/components/ui/field";
import { RECIPE_KINDS, type RecipeFields, type RecipeKind } from "@/lib/api/secrets";

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
