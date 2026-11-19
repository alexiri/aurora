"use client";

import { Field } from "@/components/ui/field";
import { SecretInput } from "@/components/ui/secret-input";

/** Write-only secret value input (SEC-12): hidden while typing, Show / Hide, never prefilled (D-375). */
export function ValueInput({
  id,
  label = "Value",
  value,
  onChange,
  error,
}: {
  id: string;
  label?: string;
  value: string;
  onChange: (v: string) => void;
  error?: string;
}) {
  // `Field` links its hint and error to the child whose id it has: `SecretInput` passes them on to the input.
  return (
    <Field id={id} label={label} error={error}>
      <SecretInput id={id} revealLabel={label.toLowerCase()} autoComplete="new-password" value={value} onChange={onChange} />
    </Field>
  );
}
