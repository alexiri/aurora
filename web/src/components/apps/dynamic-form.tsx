"use client";

import type { ReactNode } from "react";

import { Field, inputClass } from "@/components/ui/field";
import { visibleFields, type FieldValues, type FormField } from "@/lib/api/apps";
import { safeHref } from "./markdown";

export function FieldControl({ f, value, error, onChange }: { f: FormField; value: string; error?: string; onChange: (v: string) => void }) {
  const id = `cf-${f.name}`;
  const help = f.help_url ? safeHref(f.help_url) : null;
  const hint = f.placeholder ?? undefined;
  let control;
  if (f.type === "select") {
    const warn = f.options?.find((o) => o.value === value)?.warning;
    control = (
      <>
        <select id={id} className={inputClass} value={value} required={f.required} onChange={(e) => onChange(e.target.value)}>
          {!f.required || value === "" ? <option value="">Select…</option> : null}
          {(f.options ?? []).map((o) => (
            <option key={o.value} value={o.value}>{o.label}</option>
          ))}
        </select>
        {warn ? <p role="status" className="text-xs text-pending-ink">{warn}</p> : null}
      </>
    );
  } else if (f.type === "file" || f.type === "json_file") {
    control = (
      <input
        id={id}
        type="file"
        accept={f.type === "json_file" ? "application/json,.json" : undefined}
        className={inputClass}
        onChange={(e) => {
          const file = e.target.files?.[0];
          if (!file) return onChange("");
          void file.text().then(onChange);
        }}
      />
    );
  } else if (f.type === "generated_secret") {
    // Created by the server and never shown: a read-only line, not an input.
    control = (
      <p id={id} data-testid={`generated-${f.name}`} className="rounded-control border border-line bg-subtle px-3 py-2 text-sm text-ink-2">
        Generated automatically
      </p>
    );
  } else {
    const extra = f as FormField & { default?: unknown; min?: number | null; max?: number | null };
    const fallback = extra.default !== undefined && extra.default !== null ? String(extra.default) : undefined;
    control = (
      <input
        id={id}
        className={inputClass}
        type={f.type === "secret" ? "password" : f.type === "number" ? "number" : f.type === "url" ? "url" : "text"}
        inputMode={f.type === "number" ? "numeric" : undefined}
        min={f.type === "number" ? (extra.min ?? undefined) : undefined}
        max={f.type === "number" ? (extra.max ?? undefined) : undefined}
        autoComplete="off"
        placeholder={f.placeholder ?? fallback}
        required={f.required}
        value={value}
        onChange={(e) => onChange(e.target.value)}
      />
    );
  }
  return (
    <Field id={id} label={f.label + (f.required || f.type === "generated_secret" ? "" : " (optional)")} error={error} hint={f.type === "select" || f.type === "generated_secret" ? undefined : hint}>
      {control}
      {help ? (
        <a href={help} target="_blank" rel="noopener noreferrer" className="text-xs text-link underline">
          Where do I find this? ({help})
        </a>
      ) : null}
    </Field>
  );
}

/** Renders `ConnectMethod.fields` honouring `show_when`. */
export function DynamicForm({
  fields,
  values,
  errors,
  onChange,
  override,
}: {
  fields: FormField[];
  values: FieldValues;
  errors: Record<string, string>;
  onChange: (name: string, v: string) => void;
  /** Replaces the whole control of a field (e.g. a stored secret with Replace/Clear); `undefined` keeps the default. */
  override?: (f: FormField, error?: string) => ReactNode | undefined;
}) {
  return (
    <>
      {visibleFields(fields, values).map((f) => {
        const custom = override?.(f, errors[f.name]);
        if (custom !== undefined) return <div key={f.name}>{custom}</div>;
        return <FieldControl key={f.name} f={f} value={values[f.name] ?? ""} error={errors[f.name]} onChange={(v) => onChange(f.name, v)} />;
      })}
    </>
  );
}
