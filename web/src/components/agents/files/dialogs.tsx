"use client";

import { useState, type FormEvent } from "react";

import { Button } from "@/components/ui/button";
import { ConfirmDialog, FormDialog } from "@/components/ui/confirm-dialog";
import { Dialog, DialogContent } from "@/components/ui/dialog";
import { Field, inputClass } from "@/components/ui/field";
import { CheckboxField } from "@/components/ui/form";
import { Notice } from "@/components/ui/notice";
import {
  WORKSPACE_ROOT,
  baseName,
  breadcrumbs,
  filesErrorMessage,
  isManaged,
  nameProblem,
  normalizePath,
  parentPath,
  useFileList,
  type FileEntry,
} from "@/lib/api/files";

import type { ClashChoice, UploadJob } from "./upload-queue";

// Each dialog mounts its state fresh every time it opens (the wrapper renders nothing while closed), so there is no
// reset-in-an-effect: a reopened dialog never shows the last attempt's text or error.

/** **Go to path** (docs/14 §Files item 1): a path under `/workspace`, absolute or relative to the folder shown. */
export function GoToDialog({
  open,
  onOpenChange,
  current,
  onGo,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  current: string;
  onGo: (path: string) => void;
}) {
  return open ? <GoToBody onOpenChange={onOpenChange} current={current} onGo={onGo} /> : null;
}

function GoToBody({ onOpenChange, current, onGo }: { onOpenChange: (open: boolean) => void; current: string; onGo: (path: string) => void }) {
  const [value, setValue] = useState(current);
  const [error, setError] = useState<string | undefined>();
  function submit(e: FormEvent) {
    e.preventDefault();
    const path = normalizePath(value, current);
    if (!path) {
      setError(`Enter a path under ${WORKSPACE_ROOT}, without “..”.`);
      return;
    }
    onOpenChange(false);
    onGo(path);
  }
  return (
    <FormDialog open onOpenChange={onOpenChange} title="Go to path" description={`Only ${WORKSPACE_ROOT} and what is inside it can be shown.`} submitLabel="Go" onSubmit={submit}>
      <Field id="files-goto" label="Path" error={error}>
        <input id="files-goto" className={`${inputClass} font-mono`} value={value} onChange={(e) => setValue(e.target.value)} autoComplete="off" spellCheck={false} />
      </Field>
    </FormDialog>
  );
}

export type NameDialogKind = "folder" | "file" | "rename";

const COPY: Record<NameDialogKind, { title: string; submit: string; label: string }> = {
  folder: { title: "New folder", submit: "Create folder", label: "Folder name" },
  file: { title: "New file", submit: "Create file", label: "File name" },
  rename: { title: "Rename", submit: "Rename", label: "New name" },
};

/**
 * One name (not a path) for **New folder**, **New file** and **Rename** (docs/14 §Files items 2 and 5). A name that is in
 * the listing, or that the API refuses as existing (409 `file_exists`), is refused inline.
 */
export function NameDialog({
  kind,
  open,
  onOpenChange,
  initial = "",
  taken,
  onSubmit,
}: {
  kind: NameDialogKind;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  initial?: string;
  /** Names in the folder; the entry being renamed keeps its own name (a no-op rename is not an error). */
  taken: ReadonlySet<string>;
  /** Performs the action; a thrown error is shown as words under the field. */
  onSubmit: (name: string) => Promise<void>;
}) {
  return open ? <NameBody kind={kind} onOpenChange={onOpenChange} initial={initial} taken={taken} onSubmit={onSubmit} /> : null;
}

function NameBody({
  kind,
  onOpenChange,
  initial,
  taken,
  onSubmit,
}: {
  kind: NameDialogKind;
  onOpenChange: (open: boolean) => void;
  initial: string;
  taken: ReadonlySet<string>;
  onSubmit: (name: string) => Promise<void>;
}) {
  const copy = COPY[kind];
  const [value, setValue] = useState(initial);
  const [error, setError] = useState<string | undefined>();
  const [busy, setBusy] = useState(false);
  async function submit(e: FormEvent) {
    e.preventDefault();
    const name = value.trim();
    // (P5-REFINE-15) A rename refuses a slash and says where moving is.
    const problem = kind === "rename" && name.includes("/") ? "To move it, use Move to…" : nameProblem(name);
    if (problem) return setError(problem);
    if (kind === "rename" && name === initial) return onOpenChange(false);
    if (taken.has(name)) return setError("An item with that name already exists.");
    setBusy(true);
    try {
      await onSubmit(name);
      onOpenChange(false);
    } catch (err) {
      setError(filesErrorMessage(err));
      setBusy(false);
    }
  }
  return (
    <FormDialog open onOpenChange={onOpenChange} title={copy.title} submitLabel={copy.submit} busy={busy} onSubmit={(e) => void submit(e)}>
      <Field id="files-name" label={copy.label} error={error}>
        <input id="files-name" className={inputClass} value={value} onChange={(e) => setValue(e.target.value)} autoComplete="off" spellCheck={false} />
      </Field>
    </FormDialog>
  );
}

/**
 * (P5-REFINE-15, computer-4) **Leave without saving?** with **Keep editing** · **Discard** · **Save and leave**. Shown for
 * every way out of a file with unsaved changes (Back to folder, another file, the breadcrumb, the page's own links).
 */
export function LeaveDialog({
  name,
  onKeep,
  onDiscard,
  onSaveAndLeave,
}: {
  /** The file's name; null while there is nothing to ask. */
  name: string | null;
  onKeep: () => void;
  onDiscard: () => void;
  onSaveAndLeave: () => Promise<void>;
}) {
  return name ? <LeaveBody key={name} name={name} onKeep={onKeep} onDiscard={onDiscard} onSaveAndLeave={onSaveAndLeave} /> : null;
}

function LeaveBody({ name, onKeep, onDiscard, onSaveAndLeave }: { name: string; onKeep: () => void; onDiscard: () => void; onSaveAndLeave: () => Promise<void> }) {
  const [busy, setBusy] = useState(false);
  return (
    <Dialog open onOpenChange={(o) => (o ? undefined : onKeep())}>
      <DialogContent title="Leave without saving?" description={`${name} has changes that aren't saved.`}>
        <div className="mt-2 flex flex-wrap justify-end gap-2">
          <Button type="button" variant="outline" onClick={onKeep}>
            Keep editing
          </Button>
          <Button type="button" variant="danger" disabled={busy} onClick={onDiscard}>
            Discard
          </Button>
          <Button
            type="button"
            variant="primary"
            disabled={busy}
            onClick={() => {
              setBusy(true);
              void onSaveAndLeave().finally(() => setBusy(false));
            }}>
            Save and leave
          </Button>
        </div>
      </DialogContent>
    </Dialog>
  );
}

/** (P5-REFINE-15, computer-9) The first time a hidden file is opened for editing. */
export function HiddenEditDialog({ name, onCancel, onConfirm }: { name: string | null; onCancel: () => void; onConfirm: () => void }) {
  return (
    <ConfirmDialog
      open={name !== null}
      onOpenChange={(o) => !o && onCancel()}
      title="Edit a hidden file?"
      description={`${name ?? "This file"} holds settings Aurora and the agent rely on. Edit it only if you know why.`}
      confirmLabel="Edit anyway"
      onConfirm={onConfirm}
    />
  );
}

/**
 * (P5-REFINE-15, computer-3) **Move to…**: a folder tree one level at a time (a breadcrumb, folders only, `.aurora` left
 * out), **Cancel** · **Move here**. `onMove` receives the destination folder; a thrown error is shown under the list.
 */
export function MoveDialog({
  agent,
  entry,
  onOpenChange,
  onMove,
}: {
  agent: string;
  entry: FileEntry | null;
  onOpenChange: (open: boolean) => void;
  onMove: (entry: FileEntry, dir: string) => Promise<void>;
}) {
  return entry ? <MoveBody key={entry.path} agent={agent} entry={entry} onOpenChange={onOpenChange} onMove={onMove} /> : null;
}

function MoveBody({ agent, entry, onOpenChange, onMove }: { agent: string; entry: FileEntry; onOpenChange: (open: boolean) => void; onMove: (entry: FileEntry, dir: string) => Promise<void> }) {
  const here = parentPath(entry.path);
  const [dir, setDir] = useState(here);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | undefined>();
  const list = useFileList(agent, dir);
  // Folders only; `.aurora` is the platform's; a folder cannot go into itself.
  const folders = (list.data?.entries ?? []).filter((e) => e.kind === "dir" && e.path !== entry.path && !isManaged(e.path));
  const trail = breadcrumbs(dir);
  const same = dir === here;
  return (
    <Dialog open onOpenChange={onOpenChange}>
      <DialogContent title={`Move ${entry.name}`} description={`Choose a folder in ${baseName(WORKSPACE_ROOT)}.`}>
        <nav aria-label="Destination folder" className="flex flex-wrap items-center gap-x-1 text-sm">
          {trail.map((t, i) => (
            <span key={t.path} className="inline-flex items-center gap-1">
              {i > 0 ? <span aria-hidden="true" className="text-muted">/</span> : null}
              {i === trail.length - 1 ? (
                <span aria-current="page" className="font-medium text-ink">{t.label}</span>
              ) : (
                <button type="button" className="min-h-9 text-accent-ink underline underline-offset-2" onClick={() => setDir(t.path)}>
                  {t.label}
                </button>
              )}
            </span>
          ))}
        </nav>
        <ul aria-label="Folders" className="mt-2 max-h-64 divide-y divide-line overflow-y-auto rounded-control border border-line">
          {list.isPending ? <li className="px-3 py-2 text-sm text-muted">Loading…</li> : null}
          {list.isError ? <li className="px-3 py-2 text-sm text-danger-ink">{filesErrorMessage(list.error)}</li> : null}
          {list.data && folders.length === 0 ? <li className="px-3 py-2 text-sm text-muted">No folders here.</li> : null}
          {folders.map((f) => (
            <li key={f.path}>
              <button type="button" className="flex min-h-11 w-full items-center px-3 py-2 text-left text-sm hover:bg-subtle" onClick={() => setDir(f.path)}>
                {f.name}
              </button>
            </li>
          ))}
        </ul>
        {error ? (
          <Notice tone="danger" role="alert" className="mt-2">
            {error}
          </Notice>
        ) : null}
        <div className="mt-4 flex flex-wrap justify-end gap-2">
          <Button type="button" variant="outline" onClick={() => onOpenChange(false)}>
            Cancel
          </Button>
          <Button
            type="button"
            variant="primary"
            disabled={busy || same}
            onClick={async () => {
              setBusy(true);
              setError(undefined);
              try {
                await onMove(entry, dir);
                onOpenChange(false);
              } catch (e) {
                setError(filesErrorMessage(e));
                setBusy(false);
              }
            }}>
            Move here
          </Button>
        </div>
      </DialogContent>
    </Dialog>
  );
}

/** **Delete** (docs/14 §Files item 5): a `danger` confirmation; a folder says it takes everything inside it with it. */
export function DeleteDialog({
  entry,
  onOpenChange,
  onConfirm,
}: {
  entry: FileEntry | null;
  onOpenChange: (open: boolean) => void;
  onConfirm: (entry: FileEntry) => Promise<void>;
}) {
  return entry ? <DeleteBody key={entry.path} entry={entry} onOpenChange={onOpenChange} onConfirm={onConfirm} /> : null;
}

function DeleteBody({ entry, onOpenChange, onConfirm }: { entry: FileEntry; onOpenChange: (open: boolean) => void; onConfirm: (entry: FileEntry) => Promise<void> }) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | undefined>();
  return (
    <ConfirmDialog
      open
      onOpenChange={onOpenChange}
      title={`Delete ${entry.name}?`}
      description={entry.kind === "dir" ? "This also deletes everything inside the folder. It cannot be undone." : "It cannot be undone."}
      confirmLabel="Delete"
      danger
      busy={busy}
      onConfirm={async () => {
        setBusy(true);
        setError(undefined);
        try {
          await onConfirm(entry);
          onOpenChange(false);
        } catch (err) {
          setError(filesErrorMessage(err));
          setBusy(false);
        }
      }}>
      {error ? (
        <Notice tone="danger" role="alert">
          {error}
        </Notice>
      ) : null}
    </ConfirmDialog>
  );
}

/**
 * A name that already exists (docs/14 §Files item 4): **Replace**, **Keep both** or **Skip**, and "Do this for the
 * remaining N" when more uploads are waiting. Closing the dialog skips (never overwrites).
 */
export function ClashDialog({
  job,
  remaining,
  onChoose,
}: {
  job: UploadJob | null;
  remaining: number;
  onChoose: (choice: ClashChoice, applyToRest: boolean) => void;
}) {
  return job ? <ClashBody key={job.id} job={job} remaining={remaining} onChoose={onChoose} /> : null;
}

function ClashBody({ job, remaining, onChoose }: { job: UploadJob; remaining: number; onChoose: (choice: ClashChoice, applyToRest: boolean) => void }) {
  const [all, setAll] = useState(false);
  return (
    <Dialog open onOpenChange={(o) => (o ? undefined : onChoose("skip", false))}>
      <DialogContent title={`“${job.name}” already exists`} description={`In ${job.dir}. What should happen to the file you are uploading?`}>
        {remaining > 0 ? (
          <div className="mb-4">
            <CheckboxField id="files-clash-all" label={`Do this for the remaining ${remaining}`} checked={all} onChange={setAll} />
          </div>
        ) : null}
        <div className="flex flex-wrap justify-end gap-2">
          <Button type="button" variant="ghost" onClick={() => onChoose("skip", all)}>
            Skip
          </Button>
          <Button type="button" variant="outline" onClick={() => onChoose("keep", all)}>
            Keep both
          </Button>
          <Button type="button" variant="danger" onClick={() => onChoose("replace", all)}>
            Replace
          </Button>
        </div>
      </DialogContent>
    </Dialog>
  );
}
