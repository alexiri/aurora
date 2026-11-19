"use client";

import { useState } from "react";

import { Button } from "@/components/ui/button";
import { DataTable, Td, Th } from "@/components/ui/data-table";
import { Dialog, DialogClose, DialogContent } from "@/components/ui/dialog";
import { inputClass } from "@/components/ui/field";
import { ErrorNotice, Notice } from "@/components/ui/notice";
import { useImportSettings, type ImportResult } from "@/lib/api/settings";

function show(v: unknown): string {
  if (v === null || v === undefined) return "—";
  return typeof v === "string" ? v : JSON.stringify(v);
}

/**
 * Import… (C-776): paste or choose a YAML/JSON document, check it (a dry run shows each section's
 * diff, the keys ignored because Helm sets them to the same value, and errors), then Apply — enabled
 * only without errors. No re-authentication step (phase 5, FIX-157, D-317).
 */
export function ImportDialog({ open, onOpenChange }: { open: boolean; onOpenChange: (open: boolean) => void }) {
  const [text, setText] = useState("");
  const [mode, setMode] = useState<"replace" | "merge">("replace");
  const [result, setResult] = useState<ImportResult | null>(null);
  const [error, setError] = useState<unknown>(null);
  const [done, setDone] = useState(false);
  const imp = useImportSettings();
  const format = text.trimStart().startsWith("{") ? "json" : "yaml";

  async function check() {
    setError(null);
    setDone(false);
    try {
      setResult(await imp.mutateAsync({ document: text, format, dry_run: true, mode }));
    } catch (e) {
      setResult(null);
      setError(e);
    }
  }

  async function apply() {
    if (!result) return;
    setError(null);
    const generations = Object.fromEntries(result.sections.map((s) => [s.section, s.generation]));
    try {
      // The generated type of this open-shaped object is `Record<string, never>`.
      await imp.mutateAsync({ document: text, format, dry_run: false, mode, generations: generations as never });
      setDone(true);
      setResult(null);
    } catch (e) {
      setError(e);
    }
  }

  const rows = result?.sections.flatMap((s) => s.diff.map((d) => ({ section: s.section, d }))) ?? [];
  const warnings = result?.sections.flatMap((s) => s.warnings.map((w) => ({ section: s.section, w }))) ?? [];
  const loosening = result?.sections.flatMap((s) => s.loosening) ?? [];
  const errors = result?.errors ?? [];

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent
        size="xl"
        title="Import settings"
        description="Paste a settings document or choose a file. Nothing changes until you apply it. Secrets are never imported."
      >
        <div className="space-y-4">
          <input
            type="file"
            accept=".yaml,.yml,.json,application/json,application/yaml"
            aria-label="Settings file"
            className="block w-full text-sm text-ink-2"
            onChange={(e) => {
              const f = e.target.files?.[0];
              if (f) void f.text().then((t) => setText(t));
            }}
          />
          <textarea
            aria-label="Settings document"
            rows={8}
            spellCheck={false}
            className={`${inputClass} font-mono`}
            placeholder="aurora_settings: 1&#10;sections: {}"
            value={text}
            onChange={(e) => {
              setText(e.target.value);
              setResult(null);
            }}
          />
          <div className="flex flex-wrap items-center gap-3 text-sm text-ink-2">
            <label className="flex items-center gap-2">
              <input type="radio" name="import-mode" checked={mode === "replace"} onChange={() => setMode("replace")} />
              Replace the listed sections
            </label>
            <label className="flex items-center gap-2">
              <input type="radio" name="import-mode" checked={mode === "merge"} onChange={() => setMode("merge")} />
              Merge: change only the listed keys
            </label>
          </div>

          <ErrorNotice error={error} />
          {done ? (
            <Notice tone="accent" role="status">
              Imported. Applied to every replica within seconds.
            </Notice>
          ) : null}
          {errors.map((e, i) => (
            <Notice key={i} tone="danger" role="alert">
              <strong>{e.section}</strong>: {e.message}
            </Notice>
          ))}
          {loosening.length > 0 ? (
            <Notice tone="pending" role="status">
              This allows more than before: {loosening.map((l) => l.key).join(", ")}.
            </Notice>
          ) : null}
          {warnings.map((w, i) => (
            <Notice key={i} tone="neutral" role="status">
              {w.w.message}
            </Notice>
          ))}
          {result && rows.length === 0 && errors.length === 0 ? (
            <p className="text-sm text-ink-2">No changes: the document matches the current settings.</p>
          ) : null}
          {rows.length > 0 ? (
            <DataTable caption="Changes the import would make">
              <thead>
                <tr>
                  <Th>Section</Th>
                  <Th>Setting</Th>
                  <Th>Before</Th>
                  <Th>After</Th>
                </tr>
              </thead>
              <tbody>
                {rows.map(({ section, d }, i) => (
                  <tr key={i}>
                    <Td>{section}</Td>
                    <Td>
                      <code className="font-mono text-xs">{d.key}</code>
                      {d.entry_id ? ` (${d.entry_id})` : ""}
                    </Td>
                    <Td className="max-w-56 break-words">{show(d.before)}</Td>
                    <Td className="max-w-56 break-words">{show(d.after)}</Td>
                  </tr>
                ))}
              </tbody>
            </DataTable>
          ) : null}

          <div className="flex flex-wrap justify-end gap-2">
            <DialogClose asChild>
              <Button type="button" variant="outline">
                Close
              </Button>
            </DialogClose>
            <Button type="button" variant="outline" disabled={!text.trim() || imp.isPending} onClick={() => void check()}>
              Check
            </Button>
            <Button
              type="button"
              disabled={!result || errors.length > 0 || rows.length === 0 || imp.isPending}
              onClick={() => void apply()}
            >
              Apply
            </Button>
          </div>
        </div>
      </DialogContent>
    </Dialog>
  );
}
