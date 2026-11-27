"use client";

import { useEffect, useRef, useState, type FormEvent, type ReactNode } from "react";

import { setLeaveGuard } from "@/components/agents/files/leave-gate";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { Field, inputClass } from "@/components/ui/field";
import { SubScreenHeader } from "@/components/ui/list-detail";
import { MarkdownEditor } from "@/components/ui/markdown-editor";
import { ErrorNotice } from "@/components/ui/notice";
import { SaveBar } from "@/components/ui/save-bar";
import { tabId } from "@/components/ui/tabs";
import { ApiError } from "@/lib/api/client";
import { errorMessage, fieldErrors } from "@/lib/api/errors";
import {
  SKILL_BODY_MAX,
  SKILL_DESCRIPTION_MAX,
  SKILL_TITLE_MAX,
  SKILL_TOTAL_MAX,
  SkillFileError,
  skillContentSize,
  useCreateSkillWithFiles,
  useDeleteSkillFile,
  usePutSkillFile,
  useUpdateSkill,
  type NewSkillFile,
  type Skill,
  type SkillScope,
  type UpdateSkillRequest,
} from "@/lib/api/skills";
import { formatCount } from "@/lib/format-date";
import { newSkillName, skillDisplayName } from "@/lib/skill-name";

import { EditorFiles, TOTAL_TOO_LARGE, type EditorFile } from "./files";
import { isOutsideText, type SkillBody } from "./model";

const BODY_ID = "skill-body";

const encode = (text: string) => new TextEncoder().encode(text);
const chars = (text: string) => [...text].length;

/**
 * (P5-SKILLS-3, skill-4) The one form for a new and an existing skill: **Name** as typed (its folder shown small, made
 * from the name and fixed after the first save), when to use it, the instructions in the shared Write / Preview editor,
 * the files folded to one line, and the shared save bar. Files added to or deleted from a skill that exists are written
 * at once; text edited in place is saved with the form. While there are unsaved changes, every way out asks first.
 */
export function SkillEditor({
  scope,
  skill,
  taken,
  agentName,
  chip,
  narrow,
  onSaved,
  onClose,
  onBack,
}: {
  scope: SkillScope;
  /** Absent for a new skill. */
  skill?: SkillBody;
  /** The folder names already used at this level. */
  taken: string[];
  /** Whom the words are about on an agent's page; null on a workspace or organization page. */
  agentName: string | null;
  chip: ReactNode;
  narrow: boolean;
  onSaved: (id: string) => void;
  /** Leaves the form, dropping what was typed. */
  onClose: () => void;
  onBack: () => void;
}) {
  const id = skill?.id ?? "";
  const create = useCreateSkillWithFiles(scope);
  const update = useUpdateSkill(scope, id);
  const putFile = usePutSkillFile(scope, id);
  const delFile = useDeleteSkillFile(scope, id);
  const shownName = skill ? skillDisplayName(skill.name, skill.title) : "";
  const [title, setTitle] = useState(shownName);
  const [description, setDescription] = useState(skill?.description ?? "");
  const [body, setBody] = useState(skill?.body ?? "");
  const [drafts, setDrafts] = useState<Record<string, string>>({});
  const [staged, setStaged] = useState<NewSkillFile[]>([]);
  // What the files on the server are after this form's own writes, until the list is read again.
  const [saved, setSaved] = useState<Skill | null>(null);
  const [errors, setErrors] = useState<{ title?: string; description?: string; body?: string }>({});
  const [totalError, setTotalError] = useState(false);
  const [busy, setBusy] = useState(false);
  const [failure, setFailure] = useState<unknown>(null);

  const stored = saved ?? skill;
  const files: EditorFile[] = skill
    ? [...(stored?.files ?? [])].sort((a, b) => a.path.localeCompare(b.path))
    : staged.map((f) => ({ path: f.path, executable: f.executable, size: skillContentSize(f.content), content: f.content }));
  const sizeOf = (f: EditorFile) => (drafts[f.path] === undefined ? f.size : chars(drafts[f.path]!));
  const shownFiles = files.map((f) => ({ ...f, size: sizeOf(f) }));
  const total = chars(body) + shownFiles.reduce((n, f) => n + f.size, 0);
  const over = chars(body) > SKILL_BODY_MAX;
  const folder = skill ? skill.name : title.trim() ? newSkillName(title, taken) : "";
  const changed = skill
    ? title !== shownName || description !== skill.description || body !== skill.body || Object.keys(drafts).length > 0
    : Boolean(title || description || body || staged.length);
  const barDirty = !skill || changed;

  const serverFields = failure ? fieldErrors(failure) : {};
  const fileFailure = failure instanceof SkillFileError ? `“${failure.path}” couldn’t be saved, so no skill was created: ${errorMessage(failure.reason)}` : null;

  function problems() {
    const next: typeof errors = {};
    if (!title.trim()) next.title = "Give the skill a name.";
    else if (chars(title.trim()) > SKILL_TITLE_MAX) next.title = `Keep it to ${SKILL_TITLE_MAX} characters.`;
    if (!description.trim()) next.description = `Say when ${agentName ?? "agents"} should use it.`;
    else if (chars(description.trim()) > SKILL_DESCRIPTION_MAX) next.description = `Keep it to ${SKILL_DESCRIPTION_MAX} characters.`;
    if (over) next.body = `That is too long: instructions hold up to ${formatCount(SKILL_BODY_MAX)} characters.`;
    return next;
  }

  /** True when the skill was saved. */
  async function save(): Promise<boolean> {
    if (busy) return false;
    const next = problems();
    setErrors(next);
    const tooLarge = total > SKILL_TOTAL_MAX;
    setTotalError(tooLarge);
    if (Object.keys(next).length || tooLarge) return false;
    setBusy(true);
    setFailure(null);
    try {
      if (!skill) {
        const withDrafts = staged.map((f) => (drafts[f.path] === undefined ? f : { ...f, content: encode(drafts[f.path]!) }));
        // A folder name taken by a skill this list does not show yet moves to the next free one, once.
        for (let attempt = 0; ; attempt++) {
          try {
            const made = await create.mutateAsync({
              skill: { name: newSkillName(title, taken, attempt), title: title.trim(), description: description.trim(), body, enabled: true },
              files: withDrafts,
            });
            onSaved(made.id);
            return true;
          } catch (error) {
            if (attempt >= 1 || !(error instanceof ApiError && error.status === 409 && error.code === "conflict")) throw error;
          }
        }
      }
      const patch: UpdateSkillRequest = {};
      if (title.trim() !== shownName) patch.title = title.trim();
      if (description !== skill.description) patch.description = description.trim();
      if (body !== skill.body) patch.body = body;
      // What shrinks goes first, so the total never passes the limit on the way to a total that fits.
      const edited = Object.entries(drafts);
      const grows = ([path, text]: [string, string]) => chars(text) > (files.find((f) => f.path === path)?.size ?? 0);
      const write = async (list: [string, string][]) => {
        for (const [path, text] of list) {
          setSaved(await putFile.mutateAsync({ path, content: encode(text) }));
          setDrafts((d) => {
            const rest = { ...d };
            delete rest[path];
            return rest;
          });
        }
      };
      await write(edited.filter((e) => !grows(e)));
      if (Object.keys(patch).length) await update.mutateAsync(patch);
      await write(edited.filter(grows));
      onSaved(id);
      return true;
    } catch (error) {
      setFailure(error);
      return false;
    } finally {
      setBusy(false);
    }
  }

  const saveRef = useRef(save);
  const closeRef = useRef(onClose);
  useEffect(() => {
    saveRef.current = save;
    closeRef.current = onClose;
  });
  const guardName = skill ? shownName : "The new skill";
  useEffect(() => {
    if (!changed) return;
    setLeaveGuard({ name: guardName, save: () => saveRef.current(), discard: () => closeRef.current() });
    const warn = (e: BeforeUnloadEvent) => {
      e.preventDefault();
      e.returnValue = "";
    };
    window.addEventListener("beforeunload", warn);
    return () => {
      setLeaveGuard(null);
      window.removeEventListener("beforeunload", warn);
    };
  }, [changed, guardName]);

  async function addFile(file: NewSkillFile) {
    if (!skill) {
      setStaged((list) => [...list.filter((f) => f.path !== file.path), file].sort((a, b) => a.path.localeCompare(b.path)));
    } else {
      setSaved(await putFile.mutateAsync(file));
    }
    setDrafts((d) => {
      const rest = { ...d };
      delete rest[file.path];
      return rest;
    });
    setTotalError(false);
  }

  function replaceFile(path: string, content: Uint8Array) {
    if (!skill) {
      setStaged((list) => list.map((f) => (f.path === path ? { ...f, content } : f)));
      return;
    }
    setFailure(null);
    putFile.mutateAsync({ path, content }).then(setSaved, setFailure);
  }

  function deleteFile(path: string) {
    setDrafts((d) => {
      const rest = { ...d };
      delete rest[path];
      return rest;
    });
    if (!skill) {
      setStaged((list) => list.filter((f) => f.path !== path));
      return;
    }
    setFailure(null);
    delFile.mutateAsync(path).then(
      () => setSaved((s) => ({ ...((s ?? skill) as Skill), files: (s ?? skill).files.filter((f) => f.path !== path) })),
      setFailure,
    );
  }

  function editAbove() {
    document.getElementById(tabId(`${BODY_ID}-md`, "write"))?.click();
    requestAnimationFrame(() => document.getElementById(BODY_ID)?.focus());
  }

  const runner = agentName ?? "Agents";
  const heading = skill ? "Edit skill" : "New skill";
  const fieldShown = Object.keys(serverFields).some((f) => ["title", "description", "body"].includes(f));

  const form = (
    <form
      onSubmit={(e: FormEvent) => {
        e.preventDefault();
        void save();
      }}
      noValidate
      className="min-w-0 space-y-4"
      aria-label={skill ? `Edit ${shownName}` : heading}
    >
      <div className="flex flex-wrap items-center justify-between gap-2">
        <h3 className="font-display text-lg font-semibold text-ink">{heading}</h3>
        {chip}
      </div>
      <div className="space-y-1.5">
        <Field id="skill-title" label="Name" error={errors.title ?? serverFields.title}>
          <input id="skill-title" className={inputClass} value={title} maxLength={SKILL_TITLE_MAX} autoComplete="off" onChange={(e) => setTitle(e.target.value)} />
        </Field>
        {folder ? (
          <p className="text-xs text-muted">
            Folder: <code className="break-all font-mono">{folder}</code>
          </p>
        ) : null}
      </div>
      <Field
        id="skill-description"
        label={`When should ${agentName ?? "agents"} use it?`}
        error={errors.description ?? serverFields.description}
        hint={
          <>
            Finish “Use it to …”. Example: <em>write the Friday status report from Jira and GitHub</em>.
          </>
        }
      >
        <input id="skill-description" className={inputClass} value={description} autoComplete="off" onChange={(e) => setDescription(e.target.value)} />
      </Field>
      <MarkdownEditor
        id={BODY_ID}
        images={!(skill && isOutsideText(skill))}
        label="Instructions"
        value={body}
        onChange={setBody}
        hint={`Markdown · ${formatCount(chars(body))} of ${formatCount(SKILL_BODY_MAX)} characters`}
        error={errors.body ?? serverFields.body ?? (over ? `That is too long: instructions hold up to ${formatCount(SKILL_BODY_MAX)} characters.` : undefined)}
      />
      <EditorFiles
        skillId={skill?.id ?? undefined}
        version={stored?.updated_at}
        bodySize={chars(body)}
        files={shownFiles}
        drafts={drafts}
        runner={runner}
        error={totalError || total > SKILL_TOTAL_MAX ? <p role="alert" className="text-xs text-danger-ink">{TOTAL_TOO_LARGE}</p> : null}
        onDraft={(path, text) =>
          setDrafts((d) => {
            const rest = { ...d };
            if (text === null) delete rest[path];
            else rest[path] = text;
            return rest;
          })
        }
        onAdd={addFile}
        onReplace={replaceFile}
        onDelete={deleteFile}
        onEditAbove={editAbove}
      />
      {fileFailure ? <ErrorNotice error={failure} format={() => fileFailure} /> : <ErrorNotice error={fieldShown ? null : failure} />}
      {skill && !changed ? (
        <div className="flex justify-end">
          <Button type="button" variant="outline" onClick={onClose}>
            Done
          </Button>
        </div>
      ) : null}
      <SaveBar dirty={barDirty} saving={busy} saveDisabled={over} onDiscard={onClose} unsavedLabel={skill ? "Unsaved changes" : "New skill · not saved yet"} />
    </form>
  );

  return (
    <div className="min-w-0 space-y-3">
      {narrow ? <SubScreenHeader backLabel="Skills" onBack={onBack} /> : null}
      {narrow ? form : <Card>{form}</Card>}
      {/* Room for the bar fixed at the bottom of the window, so it never covers the last control. */}
      {barDirty ? <div aria-hidden="true" className="h-16" /> : null}
    </div>
  );
}
