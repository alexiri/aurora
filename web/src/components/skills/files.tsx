"use client";

import { useQuery } from "@tanstack/react-query";
import { useId, useRef, useState, type ChangeEvent, type FormEvent, type ReactNode } from "react";

import { ErrorState, Loading } from "@/components/states";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { ConfirmDialog, FormDialog } from "@/components/ui/confirm-dialog";
import { Field, inputClass } from "@/components/ui/field";
import { ChevronDownIcon, ChevronRightIcon, PlusIcon } from "@/components/ui/icons";
import { SwitchField } from "@/components/ui/switch";
import { TextViewer } from "@/components/ui/text-viewer";
import {
  SKILL_FILES_MAX,
  SKILL_TOTAL_MAX,
  buildTree,
  decodeSkillText,
  readSkillFile,
  skillContentSize,
  validateSkillPath,
  type FileTreeNode,
  type NewSkillFile,
  type SkillFile,
} from "@/lib/api/skills";
import { EMERGENCY_NO_SKILL_FILES } from "@/lib/emergency";
import { formatCount } from "@/lib/format-date";
import { cn } from "@/lib/utils";

export const SKILL_MD = "SKILL.md";

export const sizeWords = (size: number) => `${formatCount(size)} chars`;

export function ExecutableTag() {
  return <Badge className="border border-line-strong text-xs">Executable</Badge>;
}

export const TOTAL_TOO_LARGE = `A skill's instructions and files can total at most ${formatCount(SKILL_TOTAL_MAX)} characters.`;

/** One stored file's bytes (C-1099). `version` is the skill's `updated_at`, so a changed file is read again. */
export function useSkillFileContent(skillId: string | undefined, path: string | null, version: string | null | undefined) {
  return useQuery({
    queryKey: ["skills", skillId ?? "", "file", path ?? "", version ?? ""],
    queryFn: () => readSkillFile(skillId!, path!),
    enabled: !!skillId && !!path,
    retry: false,
  });
}

// ---- reading (the panel) ------------------------------------------------------------------------

function FileContent({ skillId, path, version }: { skillId: string; path: string; version: string | null | undefined }) {
  const file = useSkillFileContent(skillId, path, version);
  if (file.isPending) return <Loading />;
  if (file.isError) return <ErrorState error={file.error} onRetry={() => file.refetch()} />;
  const text = decodeSkillText(file.data);
  return text === null ? <p className="text-sm text-muted">This file isn&apos;t text, so it isn&apos;t shown.</p> : <TextViewer label={`File ${path}`} text={text} className={WRAP} />;
}

/** The panel is narrow: long lines wrap in the code view. */
const WRAP = "whitespace-pre-wrap [overflow-wrap:anywhere]";

/**
 * The files of a skill as a small tree, each opening in a read-only code view. Repository and app text is shown as it
 * is, never as markup. An app skill's files have no route to read them by, so only its `SKILL.md` opens. For an
 * emergency reader (`emergencyReader`) the files are only listed: nothing opens and no file is asked for.
 */
export function SkillFileTree({
  skillId,
  version,
  body,
  files,
  emergencyReader = false,
}: {
  skillId: string | null | undefined;
  version: string | null | undefined;
  body: string;
  files: SkillFile[];
  emergencyReader?: boolean;
}) {
  const [open, setOpen] = useState<string | null>(null);
  const headingId = useId();
  const row = (path: string, label: string, depth: number, file: SkillFile | null) => {
    // SKILL.md too, for a reader: its text is the instructions above.
    const readable = !emergencyReader && (path === SKILL_MD || !!skillId);
    const content = (
      <>
        <span className="min-w-0 flex-1 break-all font-mono text-xs text-ink">
          {path === label ? (
            label
          ) : (
            // Read out with its folder, which is on screen as the line above.
            <>
              <span className="sr-only">{path}</span>
              <span aria-hidden="true">{label}</span>
            </>
          )}
        </span>
        {file?.executable ? <ExecutableTag /> : null}
        <span className="shrink-0 text-xs text-muted">{sizeWords(file ? file.size : [...body].length)}</span>
      </>
    );
    const cls = cn("flex min-h-10 w-full min-w-0 items-center gap-2 py-2 pr-3 text-left", depth === 0 ? "pl-3" : depth === 1 ? "pl-8" : depth === 2 ? "pl-12" : "pl-16");
    return readable ? (
      <button type="button" aria-expanded={open === path} className={cn(cls, "hover:bg-subtle", open === path && "bg-subtle")} onClick={() => setOpen(open === path ? null : path)}>
        {content}
      </button>
    ) : (
      <div className={cls}>{content}</div>
    );
  };
  const nodes = (list: FileTreeNode[], depth: number): ReactNode =>
    list.map((n) => (
      <li key={n.path} className="min-w-0">
        {n.file ? (
          row(n.path, n.name, depth, n.file)
        ) : (
          <>
            <p className={cn("break-all py-2 pr-3 font-mono text-xs text-muted", depth === 0 ? "pl-3" : depth === 1 ? "pl-8" : "pl-12")}>{n.name}/</p>
            <ul>{nodes(n.children, depth + 1)}</ul>
          </>
        )}
      </li>
    ));
  // SKILL.md first, then the plain files, then the folders.
  const tree = buildTree(files).sort((a, b) => Number(!a.file) - Number(!b.file));
  return (
    <section aria-labelledby={headingId} className="space-y-2">
      <h4 id={headingId} className="text-label font-semibold uppercase tracking-label text-muted">
        Files · {files.length + 1}
      </h4>
      <ul className="divide-y divide-line overflow-hidden rounded-control border border-line">
        <li>{row(SKILL_MD, SKILL_MD, 0, null)}</li>
        {nodes(tree, 0)}
      </ul>
      {emergencyReader && files.length > 0 ? <p className="text-xs text-muted">{EMERGENCY_NO_SKILL_FILES}</p> : null}
      {open === SKILL_MD ? <TextViewer label={`File ${SKILL_MD}`} text={body} className={WRAP} /> : null}
      {open && open !== SKILL_MD && skillId ? <FileContent key={open} skillId={skillId} path={open} version={version} /> : null}
    </section>
  );
}

// ---- the Add file dialog ------------------------------------------------------------------------

/**
 * **Add a file**: its name inside the skill's folder, its content (pasted, or one uploaded) and whether the agent may
 * run it. A name the skill already has replaces that file.
 */
export function AddFileDialog({
  open,
  onOpenChange,
  files,
  bodySize,
  runner,
  onAdd,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  files: { path: string; size: number }[];
  bodySize: number;
  /** Who can run an executable file: the agent's name, or "Agents". */
  runner: string;
  onAdd: (file: NewSkillFile) => Promise<void>;
}) {
  const [path, setPath] = useState("");
  const [text, setText] = useState("");
  const [upload, setUpload] = useState<{ name: string; bytes: Uint8Array } | null>(null);
  const [executable, setExecutable] = useState(false);
  const [pathError, setPathError] = useState("");
  const [contentError, setContentError] = useState("");
  const [failed, setFailed] = useState<unknown>(null);
  const [busy, setBusy] = useState(false);
  const picker = useRef<HTMLInputElement>(null);
  const id = useId();
  const replaces = files.some((f) => f.path === path.trim());

  const reset = () => {
    setPath("");
    setText("");
    setUpload(null);
    setExecutable(false);
    setPathError("");
    setContentError("");
    setFailed(null);
  };
  const change = (next: boolean) => {
    if (!next) reset();
    onOpenChange(next);
  };

  async function picked(e: ChangeEvent<HTMLInputElement>) {
    const file = e.target.files?.[0];
    e.target.value = "";
    if (!file) return;
    setContentError("");
    // Bigger than a whole skill may be: not read at all.
    if (file.size > SKILL_TOTAL_MAX * 4) {
      setContentError(TOTAL_TOO_LARGE);
      return;
    }
    const bytes = new Uint8Array(await file.arrayBuffer());
    const asText = decodeSkillText(bytes);
    if (asText === null) setUpload({ name: file.name, bytes });
    else {
      setUpload(null);
      setText(asText);
    }
    if (!path.trim()) setPath(file.name);
  }

  async function submit(e: FormEvent) {
    e.preventDefault();
    // The dialog is a portal, but its events still bubble through React to the editor's form: adding a file must not
    // save the skill.
    e.stopPropagation();
    const p = path.trim();
    const content = upload ? upload.bytes : new TextEncoder().encode(text);
    const nameProblem = validateSkillPath(p) ?? (!replaces && files.length >= SKILL_FILES_MAX ? `A skill can have at most ${SKILL_FILES_MAX} files.` : null);
    const others = files.filter((f) => f.path !== p).reduce((n, f) => n + f.size, 0);
    const tooLarge = bodySize + others + skillContentSize(content) > SKILL_TOTAL_MAX ? TOTAL_TOO_LARGE : "";
    setPathError(nameProblem ?? "");
    setContentError(tooLarge);
    if (nameProblem || tooLarge) return;
    setBusy(true);
    setFailed(null);
    try {
      await onAdd({ path: p, content, executable });
      change(false);
    } catch (error) {
      setFailed(error);
    } finally {
      setBusy(false);
    }
  }

  return (
    <FormDialog open={open} onOpenChange={change} title="Add a file" submitLabel="Add file" size="md" busy={busy} error={failed} onSubmit={(e) => void submit(e)}>
      <Field id={`${id}-path`} label="File name" error={pathError || undefined} hint={replaces ? "Replaces the file with this name." : "Relative to the skill’s folder."}>
        <input id={`${id}-path`} className={cn(inputClass, "font-mono")} value={path} autoComplete="off" spellCheck={false} placeholder="scripts/run.sh" onChange={(e) => setPath(e.target.value)} />
      </Field>
      <div className="space-y-1.5">
        <label htmlFor={`${id}-content`} className="block text-sm font-medium text-ink">
          Content
        </label>
        {upload ? (
          <p className="flex flex-wrap items-center gap-2 rounded-control border border-line bg-subtle px-3 py-2 text-sm text-ink">
            <span className="min-w-0 break-all font-mono text-xs">{upload.name}</span>
            <span className="text-xs text-muted">not text, uploaded as it is</span>
            <Button type="button" variant="ghost" size="sm" onClick={() => setUpload(null)}>
              Paste instead
            </Button>
          </p>
        ) : (
          <div className="relative">
            <textarea
              id={`${id}-content`}
              className={cn(inputClass, "min-h-32 font-mono")}
              rows={5}
              value={text}
              spellCheck={false}
              aria-invalid={contentError ? true : undefined}
              aria-describedby={[text ? "" : `${id}-empty`, contentError ? `${id}-content-error` : ""].filter(Boolean).join(" ") || undefined}
              onChange={(e) => {
                setText(e.target.value);
                setContentError("");
              }}
            />
            {text ? null : (
              <p id={`${id}-empty`} className="pointer-events-none absolute left-3 right-3 top-2 font-mono text-sm text-muted">
                Paste the file here, or{" "}
                <button type="button" className="pointer-events-auto min-h-6 text-link underline" onClick={() => picker.current?.click()}>
                  upload one
                </button>
              </p>
            )}
          </div>
        )}
        <input ref={picker} type="file" className="sr-only" tabIndex={-1} aria-hidden="true" data-testid="add-file-upload" onChange={(e) => void picked(e)} />
        {contentError ? (
          <p id={`${id}-content-error`} role="alert" className="text-xs text-danger-ink">
            {contentError}
          </p>
        ) : null}
      </div>
      <SwitchField id={`${id}-exec`} label="Executable" hint={`${runner} can run it from the computer.`} checked={executable} onChange={setExecutable} />
    </FormDialog>
  );
}

// ---- editing (the editor) -----------------------------------------------------------------------

export interface EditorFile {
  path: string;
  size: number;
  executable: boolean;
  /** A file of a skill that does not exist yet: its bytes. */
  content?: Uint8Array;
}

/** A text file open for editing, or a binary one with its size, Replace and Delete. */
function OpenFile({
  skillId,
  version,
  file,
  draft,
  onDraft,
  onReplace,
}: {
  skillId: string | undefined;
  version: string | null | undefined;
  file: EditorFile;
  draft: string | undefined;
  onDraft: (path: string, text: string | null) => void;
  onReplace: (path: string, content: Uint8Array) => void;
}) {
  const stored = useSkillFileContent(file.content ? undefined : skillId, file.path, version);
  const picker = useRef<HTMLInputElement>(null);
  const bytes = file.content ?? stored.data;
  if (!bytes) {
    return stored.isError ? <ErrorState error={stored.error} onRetry={() => stored.refetch()} /> : <Loading />;
  }
  const original = decodeSkillText(bytes);
  if (original === null) {
    return (
      <div className="flex flex-wrap items-center gap-3 px-3 pb-3">
        <p className="text-xs text-muted">This file isn&apos;t text, so it can&apos;t be edited here.</p>
        <Button type="button" variant="outline" size="sm" aria-label={`Replace ${file.path}`} onClick={() => picker.current?.click()}>
          Replace
        </Button>
        <input
          ref={picker}
          type="file"
          className="sr-only"
          tabIndex={-1}
          aria-hidden="true"
          data-testid="replace-file-upload"
          onChange={(e) => {
            const f = e.target.files?.[0];
            e.target.value = "";
            if (f) void f.arrayBuffer().then((b) => onReplace(file.path, new Uint8Array(b)));
          }}
        />
      </div>
    );
  }
  return (
    <div className="px-3 pb-3">
      <textarea
        aria-label={`Content of ${file.path}`}
        className={cn(inputClass, "field-sizing-content max-h-96 min-h-24 font-mono text-xs")}
        spellCheck={false}
        value={draft ?? original}
        onChange={(e) => onDraft(file.path, e.target.value === original ? null : e.target.value)}
      />
    </div>
  );
}

/**
 * The editor's files, folded to one line ("Files · 2  SKILL.md, template.md") with **Add file**. Opened, each file can
 * be read and edited in place; `SKILL.md` is the instructions above.
 */
export function EditorFiles({
  skillId,
  version,
  bodySize,
  files,
  drafts,
  runner,
  error,
  onDraft,
  onAdd,
  onReplace,
  onDelete,
  onEditAbove,
}: {
  skillId: string | undefined;
  version: string | null | undefined;
  bodySize: number;
  files: EditorFile[];
  /** Text edited in place and not saved yet, by path. */
  drafts: Record<string, string>;
  runner: string;
  error?: ReactNode;
  onDraft: (path: string, text: string | null) => void;
  onAdd: (file: NewSkillFile) => Promise<void>;
  onReplace: (path: string, content: Uint8Array) => void;
  onDelete: (path: string) => void;
  onEditAbove: () => void;
}) {
  const [expanded, setExpanded] = useState(false);
  const [opened, setOpened] = useState<string | null>(null);
  const [adding, setAdding] = useState(false);
  const [deleting, setDeleting] = useState<string | null>(null);
  const panelId = useId();
  const names = [SKILL_MD, ...files.map((f) => f.path)];
  const line = "flex min-h-10 flex-wrap items-center gap-x-3 gap-y-1 px-3 py-1.5";
  const action = "min-h-9 text-sm font-medium text-link";
  return (
    <section aria-label="Files" className="rounded-control border border-line">
      <div className="flex flex-wrap items-center justify-between gap-2 px-2">
        <button type="button" className="flex min-h-11 min-w-0 flex-1 items-center gap-2 text-left" aria-expanded={expanded} aria-controls={panelId} onClick={() => setExpanded(!expanded)}>
          {expanded ? <ChevronDownIcon className="size-4 shrink-0 text-ink-2" /> : <ChevronRightIcon className="size-4 shrink-0 text-ink-2" />}
          <span className="shrink-0 text-sm font-semibold text-ink">Files · {names.length}</span>
          {expanded ? null : <span className="min-w-0 truncate text-xs text-muted">{names.join(", ")}</span>}
        </button>
        <Button type="button" variant="ghost" size="sm" onClick={() => setAdding(true)}>
          <PlusIcon className="size-4" />
          Add file
        </Button>
      </div>
      <div id={panelId} hidden={!expanded} className="border-t border-line">
        <ul className="divide-y divide-line">
          <li className={line}>
            <span className="min-w-0 flex-1 break-all font-mono text-xs text-ink">{SKILL_MD}</span>
            <span className="text-xs text-muted">{sizeWords(bodySize)}</span>
            <button type="button" className={action} onClick={onEditAbove}>
              Edit above
            </button>
          </li>
          {files.map((f) => {
            const isOpen = opened === f.path;
            return (
              <li key={f.path} className={isOpen ? "bg-subtle" : undefined}>
                <div className={line}>
                  {/* The name takes a line of its own before the actions would squeeze it. */}
                  <span className="min-w-32 flex-1 break-all font-mono text-xs text-ink">{f.path}</span>
                  {f.executable ? <ExecutableTag /> : null}
                  <span className="text-xs text-muted">{sizeWords(f.size)}</span>
                  <button type="button" className={action} aria-expanded={isOpen} aria-label={`${isOpen ? "Close" : "Open"} ${f.path}`} onClick={() => setOpened(isOpen ? null : f.path)}>
                    {isOpen ? "Close" : "Open"}
                  </button>
                  <button type="button" className="min-h-9 text-sm font-medium text-danger-ink" aria-label={`Delete ${f.path}`} onClick={() => setDeleting(f.path)}>
                    Delete
                  </button>
                </div>
                {isOpen ? <OpenFile skillId={skillId} version={version} file={f} draft={drafts[f.path]} onDraft={onDraft} onReplace={onReplace} /> : null}
              </li>
            );
          })}
        </ul>
      </div>
      {error ? <div className="px-3 pb-3">{error}</div> : null}
      <AddFileDialog open={adding} onOpenChange={setAdding} files={files} bodySize={bodySize} runner={runner} onAdd={onAdd} />
      <ConfirmDialog
        open={deleting !== null}
        onOpenChange={(o) => {
          if (!o) setDeleting(null);
        }}
        title={`Delete the file “${deleting ?? ""}”?`}
        description="It is removed from the skill. This can't be undone."
        confirmLabel="Delete file"
        danger
        onConfirm={() => {
          if (deleting) onDelete(deleting);
          setDeleting(null);
          setOpened(null);
        }}
      />
    </section>
  );
}
