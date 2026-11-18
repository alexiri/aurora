"use client";

import { useMemo, useState } from "react";

import { Button } from "@/components/ui/button";
import { Field, inputClass } from "@/components/ui/field";
import { CheckboxField } from "@/components/ui/form";
import type { Slot } from "@/lib/api/webhook";
import {
  buildFromExample,
  childPath,
  coveredBy,
  editPick,
  isPickable,
  parseExample,
  pickAt,
  SLOT_TYPES,
  unpickAt,
  type Json,
  type Picks,
} from "@/lib/webhook/logic";
import { cn } from "@/lib/utils";

// "Start from an example" (C-913): paste the body the receiver documents, click a value to make it a slot ("the agent fills
// this in"), click again to make it fixed. JSON only. The tree's nodes are buttons: focusable, toggled with Enter or Space.

const SLOT_NAME = /^[a-z][a-z0-9_]{0,39}$/;

function preview(v: Json): string {
  if (typeof v === "string") return JSON.stringify(v.length > 60 ? `${v.slice(0, 60)}…` : v);
  if (Array.isArray(v)) return v.length === 0 ? "[ ]" : `[ ${v.length} item${v.length === 1 ? "" : "s"} ]`;
  if (typeof v === "object" && v !== null) return `{ ${Object.keys(v).length} key${Object.keys(v).length === 1 ? "" : "s"} }`;
  return String(v);
}

function Node({
  label,
  value,
  path,
  picks,
  onToggle,
  depth,
}: {
  label: string;
  value: Json;
  path: string;
  picks: Picks;
  onToggle: (path: string, label: string) => void;
  depth: number;
}) {
  const picked = picks[path];
  const covered = !picked ? coveredBy(picks, path) : null;
  const pickable = isPickable(value) && !covered;
  const container = typeof value === "object" && value !== null;
  const kids: [string, Json][] = Array.isArray(value) ? value.map((v, i) => [String(i), v]) : container ? Object.entries(value as Record<string, Json>) : [];
  return (
    <li>
      <div className={cn("flex flex-wrap items-center gap-x-2 gap-y-1 py-0.5", covered && "opacity-60")}>
        <span className="min-w-0 break-all font-mono text-xs text-ink-2">{label}</span>
        <span className="min-w-0 break-all font-mono text-xs text-ink">{preview(value)}</span>
        {pickable ? (
          <button
            type="button"
            aria-pressed={!!picked}
            aria-label={`${picked ? "Make fixed" : "The agent fills in"} ${label || "the whole body"}`}
            onClick={() => onToggle(path, label)}
            className={cn(
              "min-h-9 rounded-control border px-2.5 text-xs font-medium focus-visible:outline focus-visible:outline-2 focus-visible:outline-accent",
              picked ? "border-accent bg-accent-soft text-accent-ink" : "border-line-strong bg-surface text-ink hover:bg-subtle",
            )}
          >
            {picked ? `Agent fills in: ${picked.name}` : "Agent fills in"}
          </button>
        ) : covered ? (
          <span className="text-xs text-muted">Part of {picks[covered]?.name}</span>
        ) : null}
      </div>
      {container && !picked && kids.length > 0 ? (
        <ul className="ml-3 border-l border-line pl-3">
          {kids.map(([k, v]) => (
            <Node key={k} label={Array.isArray(value) ? `[${k}]` : k} value={v} path={childPath(path, k)} picks={picks} onToggle={onToggle} depth={depth + 1} />
          ))}
        </ul>
      ) : null}
    </li>
  );
}

/** The editable details of each slot the example made: name, description, required and limits. */
function PickDetails({ root, picks, onChange }: { root: Json; picks: Picks; onChange: (p: Picks) => void }) {
  const built = useMemo(() => buildFromExample(root, picks), [root, picks]);
  const entries = Object.entries(picks);
  const names = entries.map(([, p]) => p.name);
  return (
    <ul className="space-y-3" aria-label="Values the agent fills in">
      {entries.map(([path, p], i) => {
        const slot: Slot | undefined = built.slots.find((s) => s.name === p.name);
        const type = slot?.type ?? "string";
        const bad = !SLOT_NAME.test(p.name) ? "Use a lowercase name of up to 40 letters, digits and underscores, starting with a letter." : names.indexOf(p.name) !== i ? "Another value has this name." : undefined;
        const id = `ex-${i}`;
        return (
          <li key={path} className="space-y-3 rounded-control border border-line p-3">
            <p className="text-xs text-muted">
              {SLOT_TYPES.find((t) => t.value === type)?.label}, from <span className="font-mono">{path || "the whole body"}</span>
            </p>
            <Field id={`${id}-name`} label="Name" error={bad}>
              <input id={`${id}-name`} className={inputClass} value={p.name} spellCheck={false} onChange={(e) => onChange(editPick(picks, path, { name: e.target.value }))} />
            </Field>
            <Field id={`${id}-desc`} label="What the agent should put here (optional)" hint="Up to 300 characters. The agent reads it.">
              <input id={`${id}-desc`} className={inputClass} maxLength={300} value={p.description} onChange={(e) => onChange(editPick(picks, path, { description: e.target.value }))} />
            </Field>
            {path !== "" ? (
              <CheckboxField id={`${id}-req`} label="Required" checked={p.required} onChange={(v) => onChange(editPick(picks, path, { required: v }))} />
            ) : null}
            {type === "string" || type === "string_list" ? (
              <Field id={`${id}-max`} label="Longest text (optional)">
                <input
                  id={`${id}-max`}
                  className={inputClass}
                  inputMode="numeric"
                  value={p.max_length ?? ""}
                  onChange={(e) => {
                    const n = Number(e.target.value);
                    onChange(editPick(picks, path, { max_length: e.target.value.trim() === "" || !Number.isInteger(n) || n < 1 ? undefined : Math.min(n, 65536) }));
                  }}
                />
              </Field>
            ) : null}
            {type === "string_list" ? (
              <Field id={`${id}-items`} label="Most items (optional)">
                <input
                  id={`${id}-items`}
                  className={inputClass}
                  inputMode="numeric"
                  value={p.max_items ?? ""}
                  onChange={(e) => {
                    const n = Number(e.target.value);
                    onChange(editPick(picks, path, { max_items: e.target.value.trim() === "" || !Number.isInteger(n) || n < 1 ? undefined : Math.min(n, 100) }));
                  }}
                />
              </Field>
            ) : null}
          </li>
        );
      })}
    </ul>
  );
}

/** `onApply` receives the template and slots; the editor below then shows them for any further change. */
export function ExamplePicker({ onApply, disabled = false }: { onApply: (r: { template: string; slots: Slot[] }) => void; disabled?: boolean }) {
  const [text, setText] = useState("");
  const [root, setRoot] = useState<Json | null>(null);
  const [message, setMessage] = useState<string | null>(null);
  const [picks, setPicks] = useState<Picks>({});

  function show() {
    const r = parseExample(text);
    if (!r.ok) {
      setRoot(null);
      setMessage(r.message);
      return;
    }
    setMessage(null);
    setRoot(r.value);
    setPicks({});
  }

  const built = root === null ? null : buildFromExample(root, picks);
  const invalidNames = Object.values(picks).some((p, i, all) => !SLOT_NAME.test(p.name) || all.findIndex((q) => q.name === p.name) !== i);

  return (
    <div className="space-y-4">
      <Field id="ex-text" label="The example body" error={message ?? undefined} hint="Paste the JSON the receiver's documentation shows, for example {&quot;text&quot;: &quot;Hello&quot;}.">
        <textarea id="ex-text" className={cn(inputClass, "min-h-28 font-mono text-xs")} spellCheck={false} value={text} disabled={disabled} onChange={(e) => setText(e.target.value)} />
      </Field>
      <Button type="button" variant="outline" size="sm" disabled={disabled || !text.trim()} onClick={show}>
        Show the example
      </Button>
      {root !== null ? (
        <div className="space-y-4">
          <p className="text-sm text-ink-2">
            Choose each value the agent should fill in. Everything else stays exactly as written. Choose a value again to make it fixed.
          </p>
          <ul aria-label="The example" className="space-y-0.5 overflow-hidden rounded-control border border-line bg-canvas p-3">
            <Node
              label=""
              value={root}
              path=""
              picks={picks}
              depth={0}
              onToggle={(path, label) => setPicks((cur) => (cur[path] ? unpickAt(cur, path) : pickAt(cur, path, label.replace(/^\[(\d+)\]$/, "item_$1") || "payload")))}
            />
          </ul>
          {Object.keys(picks).length > 0 ? <PickDetails root={root} picks={picks} onChange={setPicks} /> : null}
          <div>
            <p className="mb-1 text-sm font-medium text-ink">The template this makes</p>
            <pre data-testid="example-template" className="max-h-48 overflow-auto whitespace-pre-wrap break-all rounded-control border border-line bg-canvas p-3 font-mono text-xs text-ink">
              {built?.template}
            </pre>
            <p className="mt-1 text-xs text-muted">
              {typeof root === "object" && root !== null ? "" : "A single value is not a body most receivers accept; check the receiver's documentation."}
            </p>
          </div>
          <Button type="button" disabled={disabled || invalidNames || !built} onClick={() => built && onApply(built)}>
            Use this template
          </Button>
          <p className="text-xs text-muted">Using the template replaces the template and the values below.</p>
        </div>
      ) : null}
    </div>
  );
}
