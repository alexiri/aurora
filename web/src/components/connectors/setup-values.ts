import type { SetupField } from "@/lib/api/connectors";

/** `secret` and `generated_secret` fields are write-only: never part of the displayed settings. */
export const isSecretField = (f: SetupField) => f.type === "secret" || f.type === "generated_secret";

/** Form text for a stored setting (`null`/absent is the empty string). */
export const settingText = (v: unknown): string => (v === undefined || v === null ? "" : String(v));

/** The JSON value the API stores: numbers stay numbers (C-450). */
export function typedValue(f: SetupField, text: string): unknown {
  if (f.type !== "number") return text;
  const n = Number(text);
  return text.trim() !== "" && Number.isFinite(n) ? n : text;
}

export function initialValues(fields: SetupField[], settings: Record<string, unknown> | null | undefined): Record<string, string> {
  const out: Record<string, string> = {};
  for (const f of fields) if (!isSecretField(f)) out[f.name] = settingText(settings?.[f.name]);
  return out;
}

/**
 * Settings and secrets of an add-connector request from the typed values (empty strings and
 * generated secrets are left out: the API fills defaults and generates the key).
 */
export function splitSettings(
  fields: SetupField[],
  values: Record<string, string>,
): { settings: Record<string, unknown>; secrets: Record<string, string> } {
  const settings: Record<string, unknown> = {};
  const secrets: Record<string, string> = {};
  for (const f of fields) {
    const v = values[f.name] ?? "";
    if (v === "" || f.type === "generated_secret") continue;
    if (f.type === "secret") secrets[f.name] = v;
    else settings[f.name] = typedValue(f, v);
  }
  return { settings, secrets };
}

const hasDefault = (f: SetupField) => f.default !== undefined && f.default !== null;

/**
 * Client-side check of required fields (the API stays authoritative). A required non-secret
 * field with a default may stay empty; `have` says which secrets already exist or are entered.
 */
export function requiredErrors(
  fields: SetupField[],
  values: Record<string, string>,
  have: (f: SetupField) => boolean,
): Record<string, string> {
  const errs: Record<string, string> = {};
  for (const f of fields) {
    if (!f.required || f.type === "generated_secret") continue;
    const ok = f.type === "secret" ? have(f) : (values[f.name] ?? "").trim() !== "" || hasDefault(f);
    if (!ok) errs[f.name] = `${f.label} is required.`;
  }
  return errs;
}
