"use client";

import { useEffect, useRef, useState, type FormEvent } from "react";

import { setLeaveGuard } from "@/components/agents/files/leave-gate";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { CopyChip } from "@/components/ui/copy-value";
import { Field, inputClass } from "@/components/ui/field";
import { SubScreenHeader } from "@/components/ui/list-detail";
import { MarkdownEditor } from "@/components/ui/markdown-editor";
import { ErrorNotice } from "@/components/ui/notice";
import { SaveBar } from "@/components/ui/save-bar";
import { SwitchField } from "@/components/ui/switch";
import { useUndoToast } from "@/components/ui/undo-toast";
import { ApiError } from "@/lib/api/client";
import { fieldErrors } from "@/lib/api/errors";
import {
  MEMORY_CONTENT_MAX,
  MEMORY_DESCRIPTION_MAX,
  MEMORY_TITLE_MAX,
  useCreateMemory,
  usePutMemory,
  useSetMemoryLocked,
  type Memory,
} from "@/lib/api/memory";
import { keyProblem, memoryName, newMemoryKey } from "@/lib/memory-key";
import { cn } from "@/lib/utils";

import { lockedWords } from "./memory-view";
import { SizeMeter } from "./size-meter";

const KEY_TAKEN = "A memory with this key already exists";

const keyTaken = (e: unknown) => e instanceof ApiError && e.code === "memory_exists";

/**
 * (P5-MEMORY-2, M-2, M-4, M-8) The one form for a new and an existing memory: Title, Key (made from the title until the
 * person changes it; fixed after the first save), Description, the text in the Write / Preview editor, one **Locked**
 * switch and the shared save bar. On an existing memory the switch saves on its own (a lock is not a revision); on a new
 * one it is part of the create. While there are unsaved changes, every way out asks first.
 */
export function MemoryEditor({
  agentId,
  agentName,
  memory,
  draft,
  taken,
  narrow,
  onDirtyChange,
  onSaved,
  onClose,
  onBack,
}: {
  agentId: string;
  agentName: string;
  /** Absent for a new memory. */
  memory?: Memory;
  /** What the form starts with instead, as unsaved changes. A key that is taken is dropped. */
  draft?: { key?: string | null; title?: string; content: string };
  /** The agent's keys, so a made key is not one of them. */
  taken: string[];
  narrow: boolean;
  onDirtyChange: (dirty: boolean) => void;
  onSaved: (key: string) => void;
  /** Leaves the form, dropping what was typed. */
  onClose: () => void;
  /** The way back on a narrow screen (it asks first when something is unsaved). */
  onBack: () => void;
}) {
  const create = useCreateMemory(agentId);
  const put = usePutMemory(agentId, memory?.key ?? "");
  const lock = useSetMemoryLocked(agentId, memory?.key ?? "");
  const toast = useUndoToast();
  const [title, setTitle] = useState(memory?.title ?? draft?.title?.slice(0, MEMORY_TITLE_MAX) ?? "");
  const [description, setDescription] = useState(memory?.description ?? "");
  const [content, setContent] = useState(draft?.content ?? memory?.content ?? "");
  const [typedKey, setTypedKey] = useState<string | null>(() => (!memory && draft?.key && !taken.includes(draft.key) ? draft.key : null));
  const [lockNew, setLockNew] = useState(false);
  const [locked, setLocked] = useState(memory?.locked ?? false);
  const [contentError, setContentError] = useState("");
  const [keyError, setKeyError] = useState("");

  const madeKey = title.trim() || content.trim() ? newMemoryKey(title, content, taken) : "";
  const key = typedKey ?? madeKey;
  const over = content.length > MEMORY_CONTENT_MAX;
  const saving = create.isPending || put.isPending;
  const changed = memory
    ? title !== (memory.title ?? "") || description !== (memory.description ?? "") || content !== memory.content
    : Boolean(title || description || content || typedKey !== null || lockNew);
  // A new memory shows the bar from the start ("not saved yet"); an existing one only once something changed.
  const barDirty = !memory || changed;
  const failure = create.error ?? put.error;
  const serverFields = failure ? fieldErrors(failure) : {};
  // The field's own sentence says what is wrong; anything else is the notice above the bar.
  const shownOnField = keyError !== "" || Object.keys(serverFields).some((f) => ["title", "description", "content", "key"].includes(f));

  useEffect(() => {
    onDirtyChange(changed);
  }, [changed, onDirtyChange]);
  useEffect(() => () => onDirtyChange(false), [onDirtyChange]);

  /** True when the memory was saved. */
  async function save(): Promise<boolean> {
    if (saving) return false;
    const empty = !content.trim() ? `Write what ${agentName} should remember.` : "";
    setContentError(empty);
    const bad = memory ? null : typedKey !== null || key ? keyProblem(key) : null;
    setKeyError(bad ?? "");
    if (empty || bad || over) return false;
    const body = { title: title.trim() || null, description: description.trim() || null, content };
    try {
      if (memory) {
        await put.mutateAsync(body);
        onSaved(memory.key);
        return true;
      }
      // A made key that is already taken (a memory this list does not show yet) moves to the next free one once; a key
      // the person chose is never changed behind their back.
      for (let attempt = 0; ; attempt++) {
        const next = typedKey ?? newMemoryKey(title, content, taken, attempt);
        try {
          await create.mutateAsync({ key: next, ...body, ...(lockNew ? { locked: true } : {}) });
          onSaved(next);
          return true;
        } catch (error) {
          if (typedKey === null && attempt < 1 && keyTaken(error)) continue;
          if (keyTaken(error)) setKeyError(KEY_TAKEN);
          return false;
        }
      }
    } catch {
      return false; // shown below
    }
  }

  const saveRef = useRef(save);
  const closeRef = useRef(onClose);
  useEffect(() => {
    saveRef.current = save;
    closeRef.current = onClose;
  });
  const guardName = memory ? memoryName(memory) : "The new memory";
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

  /** The switch of an existing memory saves on its own; the toast names what happened. */
  async function setLockedTo(on: boolean) {
    const before = locked;
    setLocked(on);
    try {
      await lock.mutateAsync(on);
      toast(on ? "Locked" : "Unlocked", undefined, { success: true });
    } catch {
      setLocked(before);
    }
  }

  function submit(e: FormEvent) {
    e.preventDefault();
    void save();
  }

  const lockLine = memory && locked ? lockedWords(memory) : null;
  const shownKeyError = keyError || serverFields.key || "";

  const form = (
    <form onSubmit={submit} noValidate className="min-w-0 space-y-4" aria-label={memory ? `Edit ${memoryName(memory)}` : "New memory"}>
      <Field id="mem-title" label="Title" error={serverFields.title}>
        <input id="mem-title" className={inputClass} value={title} maxLength={MEMORY_TITLE_MAX} onChange={(e) => setTitle(e.target.value)} />
      </Field>
      {memory ? (
        <div className="flex flex-wrap items-center gap-x-3 gap-y-1">
          <CopyChip label="Key" value={memory.key} />
          <p className="min-w-0 flex-1 basis-40 text-xs text-muted">{agentName} finds the memory by this name. It can&apos;t be changed.</p>
        </div>
      ) : (
        <div className="space-y-1.5">
          <label htmlFor="mem-key" className="block text-sm font-medium text-ink">
            Key
          </label>
          <div className="relative">
            <input
              id="mem-key"
              className={cn(inputClass, "font-mono", typedKey === null && "pr-36")}
              value={key}
              autoComplete="off"
              spellCheck={false}
              aria-invalid={shownKeyError ? true : undefined}
              aria-describedby={shownKeyError ? "mem-key-error" : "mem-key-hint"}
              onChange={(e) => {
                setTypedKey(e.target.value);
                setKeyError("");
                create.reset();
              }}
            />
            {typedKey === null ? (
              <span className="pointer-events-none absolute right-3 top-1/2 -translate-y-1/2 text-xs text-muted">Made from the title</span>
            ) : null}
          </div>
          {shownKeyError ? (
            <p id="mem-key-error" role="alert" className="text-xs text-danger-ink">
              {shownKeyError}
            </p>
          ) : (
            <p id="mem-key-hint" className="text-xs text-muted">
              {agentName} finds the memory by this name. Change it now if you like: lowercase words joined by dashes. It can&apos;t be changed after saving.
            </p>
          )}
        </div>
      )}
      <Field id="mem-description" label="Description" error={serverFields.description} hint={`${agentName} sees the title and this line first and opens the memory only when it fits.`}>
        <input
          id="mem-description"
          className={inputClass}
          value={description}
          maxLength={MEMORY_DESCRIPTION_MAX}
          placeholder={`One line: when should ${agentName} use this?`}
          onChange={(e) => setDescription(e.target.value)}
        />
      </Field>
      <MarkdownEditor
        id="mem-content"
        label={`What should ${agentName} remember?`}
        value={content}
        onChange={setContent}
        placeholder="Write it the way you would tell a colleague. Plain words are fine."
        error={contentError || serverFields.content}
      />
      <SizeMeter quiet label="Size" used={content.length} max={MEMORY_CONTENT_MAX} />
      <div className="space-y-1 border-t border-line pt-4">
        <SwitchField
          id="mem-locked"
          label="Locked"
          hint={`${agentName} can read this memory but can't change or delete it. You can still edit it.`}
          checked={memory ? locked : lockNew}
          onChange={(on) => (memory ? void setLockedTo(on) : setLockNew(on))}
          disabled={lock.isPending}
        />
        {lockLine ? <p className="text-xs text-muted">{lockLine}</p> : null}
      </div>
      <ErrorNotice error={lock.error ?? (shownOnField ? null : failure)} />
      {memory && !changed ? (
        <div className="flex justify-end">
          <Button type="button" variant="outline" onClick={onClose}>
            Done
          </Button>
        </div>
      ) : null}
      <SaveBar dirty={barDirty} saving={saving} saveDisabled={over} onDiscard={onClose} unsavedLabel={memory ? "Unsaved changes" : "New memory · not saved yet"} />
    </form>
  );

  return (
    <div className="min-w-0 space-y-3">
      {narrow ? <SubScreenHeader backLabel="Memory" onBack={onBack} /> : null}
      {narrow ? form : <Card>{form}</Card>}
      {/* Room for the bar fixed at the bottom of the window, so it never covers the last control. */}
      {barDirty ? <div aria-hidden="true" className="h-16" /> : null}
    </div>
  );
}
