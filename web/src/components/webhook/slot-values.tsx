"use client";

import { Field, inputClass } from "@/components/ui/field";
import { CheckboxField } from "@/components/ui/form";
import { Select } from "@/components/ui/select";
import type { Slot } from "@/lib/api/webhook";
import { cn } from "@/lib/utils";

/** A values form generated from the slots: the preview's sample values and Send test's values (C-913). */
export function SlotValues({
  slots,
  inputs,
  errors,
  onChange,
  idp,
}: {
  slots: Slot[];
  inputs: Record<string, string>;
  errors: Record<string, string>;
  onChange: (name: string, value: string) => void;
  idp: string;
}) {
  if (slots.length === 0) return <p className="text-sm text-ink-2">This webhook has no values to fill in.</p>;
  return (
    <div className="space-y-3">
      {slots.map((s, i) => {
        const id = `${idp}-${i}`;
        const value = inputs[s.name] ?? "";
        const label = `${s.name}${s.required ? "" : " (optional)"}`;
        const hint = s.description || undefined;
        if (s.type === "boolean") {
          return <CheckboxField key={s.name} id={id} label={label} hint={errors[s.name] ?? hint} checked={value === "true"} onChange={(v) => onChange(s.name, String(v))} />;
        }
        if (s.type === "enum") {
          return (
            <Field key={s.name} id={id} label={label} hint={hint} error={errors[s.name]}>
              <Select id={id} value={value} onChange={(v) => onChange(s.name, v)} options={(s.enum ?? []).map((o) => ({ value: o, label: o }))} />
            </Field>
          );
        }
        const multi = s.type === "string_list" || s.type === "json" || (s.type === "string" && (s.max_length ?? 0) > 200);
        return (
          <Field key={s.name} id={id} label={label} hint={s.type === "string_list" ? (hint ? `${hint} One per line.` : "One per line.") : hint} error={errors[s.name]}>
            {multi ? (
              <textarea id={id} className={cn(inputClass, "min-h-20", s.type === "json" && "font-mono text-xs")} spellCheck={false} value={value} onChange={(e) => onChange(s.name, e.target.value)} />
            ) : (
              <input
                id={id}
                className={inputClass}
                inputMode={s.type === "integer" || s.type === "number" ? "decimal" : undefined}
                value={value}
                onChange={(e) => onChange(s.name, e.target.value)}
              />
            )}
          </Field>
        );
      })}
    </div>
  );
}
