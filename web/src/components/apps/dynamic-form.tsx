"use client";

import { Field, inputClass } from "@/components/ui/field";
import { visibleFields, type FieldValues, type FormField } from "@/lib/api/apps";
import { safeHref } from "./markdown";

function FieldControl({ f, value, error, onChange }: { f: FormField; value: string; error?: string; onChange: (v: string) => void }) {
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
  } else {
    control = (
      <input
        id={id}
        className={inputClass}
        type={f.type === "secret" ? "password" : "text"}
        autoComplete="off"
        placeholder={f.placeholder ?? undefined}
        required={f.required}
        value={value}
        onChange={(e) => onChange(e.target.value)}
      />
    );
  }
  return (
    <Field id={id} label={f.label + (f.required ? "" : " (optional)")} error={error} hint={f.type === "select" ? undefined : hint}>
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
}: {
  fields: FormField[];
  values: FieldValues;
  errors: Record<string, string>;
  onChange: (name: string, v: string) => void;
}) {
  return (
    <>
      {visibleFields(fields, values).map((f) => (
        <FieldControl key={f.name} f={f} value={values[f.name] ?? ""} error={errors[f.name]} onChange={(v) => onChange(f.name, v)} />
      ))}
    </>
  );
}
