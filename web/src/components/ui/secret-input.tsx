"use client";

import { useState, type InputHTMLAttributes } from "react";

import { Button } from "@/components/ui/button";
import { inputClass } from "@/components/ui/field";
import { cn } from "@/lib/utils";

type Passed = Omit<InputHTMLAttributes<HTMLInputElement>, "type" | "value" | "onChange" | "children">;

/**
 * A value that is written but never read back (D-375, SEC-12): hidden while typing, with one Show / Hide button
 * beside it that reveals only what was typed or pasted here. Every secret field uses it; a stored secret is never
 * put in its `value`.
 *
 * It hides again on its own when it unmounts, when the parent clears the value to "" (a form reset after a save; a
 * person deleting everything to retype keeps it shown) and when it is disabled. No timer.
 */
export function SecretInput({
  value,
  onChange,
  revealLabel,
  autoComplete = "off",
  className,
  disabled,
  id,
  "aria-label": ariaLabel,
  ...rest
}: Passed & {
  value: string;
  onChange: (value: string) => void;
  /** Names the button for a screen reader: "Show <revealLabel>". Defaults to the input's `aria-label`. */
  revealLabel?: string;
}) {
  const [shown, setShown] = useState(false);
  // The last value this input emitted, and the value seen last render: a "" that this input did not emit came from
  // the parent (a form reset), which hides the value again. State adjusted while rendering, as React documents.
  const [emitted, setEmitted] = useState(value);
  const [seen, setSeen] = useState(value);
  if (seen !== value) {
    setSeen(value);
    setEmitted(value);
    if (value === "" && emitted !== "") setShown(false);
  }
  if (disabled && shown) setShown(false);

  const visible = shown && !disabled;
  const name = revealLabel ?? ariaLabel;
  const verb = visible ? "Hide" : "Show";
  return (
    <div className="flex gap-2">
      <input
        {...rest}
        id={id}
        type={visible ? "text" : "password"}
        autoComplete={autoComplete}
        spellCheck={false}
        autoCapitalize="off"
        autoCorrect="off"
        aria-label={ariaLabel}
        disabled={disabled}
        className={cn(inputClass, "min-w-0 flex-1", className)}
        value={value}
        onChange={(e) => {
          setEmitted(e.target.value);
          onChange(e.target.value);
        }}
      />
      <Button
        type="button"
        variant="outline"
        size="sm"
        className="shrink-0"
        disabled={disabled}
        aria-pressed={visible}
        aria-controls={id}
        aria-label={name ? `${verb} ${name}` : undefined}
        onClick={() => setShown((s) => !s)}
      >
        {verb}
      </Button>
    </div>
  );
}
