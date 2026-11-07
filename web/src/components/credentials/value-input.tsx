"use client";

import { useState } from "react";

import { Button } from "@/components/ui/button";
import { Field, inputClass } from "@/components/ui/field";

/** Write-only secret value input (SEC-12): password type, show toggle, never prefilled. */
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
  const [show, setShow] = useState(false);
  return (
    <Field id={id} label={label} error={error}>
      <div className="flex gap-2">
        <input
          id={id}
          type={show ? "text" : "password"}
          autoComplete="new-password"
          spellCheck={false}
          className={inputClass}
          value={value}
          onChange={(e) => onChange(e.target.value)}
        />
        <Button type="button" variant="outline" size="sm" aria-pressed={show} onClick={() => setShow((s) => !s)}>
          {show ? "Hide" : "Show"}
        </Button>
      </div>
    </Field>
  );
}
