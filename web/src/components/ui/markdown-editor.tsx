"use client";

import { useState } from "react";

import { cn } from "@/lib/utils";

import { Markdown } from "./markdown";
import { Tabs, tabId, tabPanelId } from "./tabs";

/** The hint under the Write / Preview tabs. */
export const MARKDOWN_HINT = "Markdown works: # headings, - lists, - [ ] checklists, tables";

type Mode = "write" | "preview";
const MODES = [
  { value: "write", label: "Write" },
  { value: "preview", label: "Preview" },
];

/**
 * (P5-MEMORY-2) The one Write / Preview editor for Markdown text: a labelled box with the tabs **Write** and
 * **Preview** (a real tablist, arrow keys), the hint line beside them, a monospace textarea that is at least 25rem tall
 * and grows with its text (no inner scrollbar), and the same Markdown renderer as everywhere for the preview. The box's
 * border shows focus (D-393). `id` is the textarea's, so the label and the error point at it.
 */
export function MarkdownEditor({
  id,
  label,
  value,
  onChange,
  placeholder,
  error,
  hint = MARKDOWN_HINT,
  className,
  images = true,
  reading = false,
}: {
  /** `false` when the text came from outside (`Markdown`'s own option): the preview fetches nothing. */
  images?: boolean;
  /** The preview of a document read at length (`Markdown`'s `reading`): drawn as its reading view draws it. */
  reading?: boolean;
  id: string;
  label: string;
  value: string;
  onChange: (value: string) => void;
  placeholder?: string;
  error?: string;
  hint?: string;
  className?: string;
}) {
  const [mode, setMode] = useState<Mode>("write");
  const base = `${id}-md`;
  return (
    <div className={cn("space-y-1.5", className)}>
      <label htmlFor={id} className="block text-sm font-medium text-ink">
        {label}
      </label>
      <div
        data-testid="markdown-editor"
        className={cn(
          "overflow-hidden rounded-control border bg-surface focus-within:border-accent dark:bg-canvas",
          error ? "border-danger-ink" : "border-line-strong",
        )}
      >
        <div className="flex flex-wrap items-center justify-between gap-x-3 border-b border-line px-2">
          <Tabs
            variant="underline"
            items={MODES}
            value={mode}
            onValueChange={(v) => setMode(v as Mode)}
            label={`${label}: Write or Preview`}
            idBase={base}
            className="!w-auto !border-b-0"
          />
          <p id={`${id}-hint`} className="py-1.5 text-xs text-muted">
            {hint}
          </p>
        </div>
        {/* Both panels exist, so each tab's `aria-controls` points at something; the other one is hidden. */}
        <div role="tabpanel" id={tabPanelId(base, "write")} aria-labelledby={tabId(base, "write")} hidden={mode !== "write"}>
          <textarea
            id={id}
            value={value}
            onChange={(e) => onChange(e.target.value)}
            placeholder={placeholder}
            aria-describedby={[`${id}-hint`, error ? `${id}-error` : ""].filter(Boolean).join(" ")}
            aria-invalid={error ? true : undefined}
            className="block min-h-100 w-full resize-y bg-transparent px-3 py-3 font-mono text-sm text-ink placeholder:text-muted field-sizing-content focus-visible:outline-none"
          />
        </div>
        <div
          role="tabpanel"
          id={tabPanelId(base, "preview")}
          aria-labelledby={tabId(base, "preview")}
          hidden={mode !== "preview"}
          tabIndex={0}
          className={cn("min-h-100 px-3 py-3", reading && "md-reading")}
        >
          {mode !== "preview" ? null : value.trim() ? <Markdown source={value} images={images} reading={reading} /> : <p className="text-sm text-muted">Nothing to preview yet.</p>}
        </div>
      </div>
      {error ? (
        <p id={`${id}-error`} role="alert" className="text-xs text-danger-ink">
          {error}
        </p>
      ) : null}
    </div>
  );
}
