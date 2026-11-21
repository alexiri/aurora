"use client";

import { useState, type FormEvent, type ReactNode } from "react";

import { Button } from "@/components/ui/button";
import { Dialog, DialogClose, DialogContent } from "@/components/ui/dialog";
import { inputClass } from "@/components/ui/field";
import { ErrorNotice, Notice } from "@/components/ui/notice";

/** True when what was typed is the visible name: case-sensitive, trimmed. */
export function matchesName(typed: string, name: string): boolean {
  return typed.trim() === name.trim() && name.trim() !== "";
}

interface TypeToConfirmProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  title: string;
  /** The red line: what goes, and that it can't be undone. */
  consequence: ReactNode;
  /** "workspace", "organization": the word in "Type the workspace name to confirm". */
  noun: string;
  /** The visible name the person types. */
  name: string;
  /** (P5-REFINE-19) The sentence before the name when it is not "Type the <noun> name to confirm" (a bulk delete types the count). */
  typeLabel?: string;
  confirmLabel: string;
  run: () => Promise<unknown>;
  onDone: () => void;
}

/**
 * (P5-REFINE-2, D-405; third rung of the confirm ladder, workspace-8) A permanent action: a dialog with a red
 * consequence line, then "Type the <noun> name to confirm: **<visible name>**" (never a slug) and a danger button
 * (`confirmLabel`, e.g. "Delete workspace") that stays disabled until the typed text matches. `run` does the delete and
 * rejects with the API's error, which shows in the dialog; on success `onDone` runs (the dialog closes).
 */
export function TypeToConfirm({ open, onOpenChange, title, ...form }: TypeToConfirmProps) {
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent title={title}>
        {/* Mounted only while open, so what was typed and any error start over each time. */}
        <ConfirmForm onOpenChange={onOpenChange} {...form} />
      </DialogContent>
    </Dialog>
  );
}

function ConfirmForm({
  onOpenChange,
  consequence,
  noun,
  name,
  typeLabel,
  confirmLabel,
  run,
  onDone,
}: Omit<TypeToConfirmProps, "open" | "title">) {
  const [typed, setTyped] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<unknown>(null);
  const id = `type-to-confirm-${noun.replace(/\s+/g, "-")}`;
  async function submit(e: FormEvent) {
    e.preventDefault();
    if (!matchesName(typed, name) || busy) return;
    setBusy(true);
    setError(null);
    try {
      await run();
      onOpenChange(false);
      onDone();
    } catch (err) {
      setError(err);
      setBusy(false);
    }
  }
  return (
    <form onSubmit={(e) => void submit(e)} className="space-y-4">
      <Notice tone="danger">{consequence}</Notice>
      <div className="space-y-1.5">
        <label htmlFor={id} className="block text-sm font-medium text-ink">
          {typeLabel ?? `Type the ${noun} name to confirm`}: <span className="font-mono">{name}</span>
        </label>
        <input
          id={id}
          className={inputClass}
          value={typed}
          autoComplete="off"
          autoCapitalize="off"
          spellCheck={false}
          onChange={(e) => setTyped(e.target.value)}
        />
      </div>
      <div className="min-h-6">
        <ErrorNotice error={error} />
      </div>
      <div className="flex flex-wrap justify-end gap-2">
        <DialogClose asChild>
          <Button type="button" variant="outline">
            Cancel
          </Button>
        </DialogClose>
        <Button type="submit" variant="danger" disabled={busy || !matchesName(typed, name)}>
          {busy ? "Deleting…" : confirmLabel}
        </Button>
      </div>
    </form>
  );
}

/** The button of a `DangerZone` that opens a `TypeToConfirm`: the zone keeps its own words, the dialog does the asking. */
export function DeleteWithName({
  buttonLabel,
  size,
  ...dialog
}: { buttonLabel: string; size?: "lg" } & Omit<TypeToConfirmProps, "open" | "onOpenChange" | "confirmLabel"> & { confirmLabel?: string }) {
  const [open, setOpen] = useState(false);
  return (
    <>
      <Button type="button" variant="danger" size={size} onClick={() => setOpen(true)}>
        {buttonLabel}
      </Button>
      <TypeToConfirm {...dialog} confirmLabel={dialog.confirmLabel ?? buttonLabel} open={open} onOpenChange={setOpen} />
    </>
  );
}
