"use client";

import { Field, inputClass } from "@/components/ui/field";

import { type TemplateInput } from "./words";

/**
 * The About step of the four-step page (C-1116): the agent's name, an optional handle, and the template's inputs —
 * each with its label, help line and default. A `choice` input is a select of the manifest's options; a `text` input
 * is a line. Presentational: the page owns the values.
 */
export function AboutStep({
  name,
  onName,
  handle,
  onHandle,
  inputs,
  values,
  onValue,
  errors = {},
}: {
  name: string;
  onName: (v: string) => void;
  handle: string;
  onHandle: (v: string) => void;
  inputs: TemplateInput[];
  values: Record<string, string>;
  onValue: (id: string, v: string) => void;
  errors?: Record<string, string>;
}) {
  return (
    <div className="space-y-4">
      <Field id="template-agent-name" label="Name" error={errors.name}>
        <input
          id="template-agent-name"
          className={inputClass}
          value={name}
          maxLength={80}
          placeholder="Rex"
          onChange={(e) => onName(e.target.value)}
        />
      </Field>
      <Field id="template-agent-handle" label="Handle (optional)" hint="Cannot be changed later." error={errors.handle}>
        <input id="template-agent-handle" className={inputClass} value={handle} onChange={(e) => onHandle(e.target.value)} />
      </Field>
      {inputs.map((input) => {
        const id = `template-input-${input.id}`;
        const label = input.required === false ? `${input.label} (optional)` : input.label;
        return (
          <Field key={input.id} id={id} label={label} hint={input.help ?? undefined} error={errors[`inputs.${input.id}`]}>
            {input.kind === "choice" ? (
              <select
                id={id}
                className={inputClass}
                value={values[input.id] ?? input.default ?? ""}
                onChange={(e) => onValue(input.id, e.target.value)}
              >
                {input.required !== false && !input.default ? <option value="">Choose…</option> : null}
                {(input.options ?? []).map((o) => (
                  <option key={o} value={o}>
                    {o}
                  </option>
                ))}
              </select>
            ) : (
              <input
                id={id}
                className={inputClass}
                value={values[input.id] ?? ""}
                maxLength={input.max_length ?? 100}
                onChange={(e) => onValue(input.id, e.target.value)}
              />
            )}
          </Field>
        );
      })}
    </div>
  );
}
