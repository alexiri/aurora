"use client";

import dynamic from "next/dynamic";
import { useId, useRef } from "react";

import { Loading } from "@/components/states";
import { cn } from "@/lib/utils";

// The shared code field (P5-WEBHOOK-4, C-913): the Files viewer's CodeMirror editor (its own lazy chunk, the dashboard's
// tokens as its theme, text only, never rendered as HTML) as a labelled form control with an error line. The editor
// holds its document itself: `docKey` remounts it with a new `initial` (a preset chosen, an example applied).
const CodeEditor = dynamic(() => import("@/components/agents/files/viewer/editor"), { ssr: false, loading: () => <Loading label="Loading editor…" /> });

const ESCAPE_CHORD_MS = 1500;

/** Moves focus to the next control after `el` that is not inside it (Esc, then Tab leaves the editor: WCAG 2.1.2). */
function focusAfter(el: HTMLElement) {
  const sel = 'a[href],button:not([disabled]),input:not([disabled]),select:not([disabled]),textarea:not([disabled]),[tabindex]:not([tabindex="-1"])';
  const next = Array.from(document.querySelectorAll<HTMLElement>(sel)).find(
    (e) => !el.contains(e) && !!(el.compareDocumentPosition(e) & Node.DOCUMENT_POSITION_FOLLOWING),
  );
  next?.focus();
}

export function CodeField({
  label,
  initial,
  docKey,
  fileName = "template.json",
  onChange,
  error,
  hint,
  readOnly = false,
  className,
}: {
  label: string;
  /** The document when the editor mounts (and again whenever `docKey` changes). */
  initial: string;
  docKey: string | number;
  /** Selects the language by extension (`.json`, `.yaml`, `.txt`). */
  fileName?: string;
  onChange: (text: string) => void;
  error?: string;
  hint?: string;
  readOnly?: boolean;
  className?: string;
}) {
  const id = useId();
  const lastEscape = useRef(0);
  return (
    <div className={cn("space-y-1.5", className)}>
      <p id={`${id}-label`} className="text-sm font-medium text-ink">
        {label}
      </p>
      <div
        data-testid="code-field"
        aria-describedby={error ? `${id}-error` : hint ? `${id}-hint` : undefined}
        className={cn("h-56 min-h-40 overflow-hidden rounded-control border bg-surface", error ? "border-danger-ink" : "border-line-strong")}
        onKeyDownCapture={(e) => {
          if (e.key === "Escape") lastEscape.current = Date.now();
          else if (e.key === "Tab" && Date.now() - lastEscape.current < ESCAPE_CHORD_MS) {
            lastEscape.current = 0;
            e.preventDefault();
            e.stopPropagation();
            focusAfter(e.currentTarget);
          }
        }}
      >
        <CodeEditor key={docKey} initial={initial} lineEnding="lf" name={fileName} readOnly={readOnly} label={label} onChange={onChange} onSave={() => undefined} />
      </div>
      {error ? (
        <p id={`${id}-error`} role="alert" className="text-xs text-danger-ink">
          {error}
        </p>
      ) : (
        <p id={`${id}-hint`} className="text-xs text-muted">
          {hint ? `${hint} ` : ""}Tab indents. Press Esc, then Tab to leave the editor.
        </p>
      )}
    </div>
  );
}
