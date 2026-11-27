"use client";

import { useRef, useState } from "react";

import { Loading } from "@/components/states";
import { Button } from "@/components/ui/button";
import { Dialog, DialogClose, DialogContent } from "@/components/ui/dialog";
import { ErrorNotice, Notice } from "@/components/ui/notice";
import { errorMessage } from "@/lib/api/errors";
import { SkillFileError, useCreateSkillWithFiles, type SkillScope } from "@/lib/api/skills";
import { skillDisplayName } from "@/lib/skill-name";
import { readSkillFolder, type FolderResult, type PickedFile } from "@/lib/skill-folder";

import { ExecutableTag, sizeWords } from "./files";

/** The files of a folder input, with their paths from the folder's parent. */
function pickedFiles(list: FileList): PickedFile[] {
  return Array.from(list).map((f) => ({
    path: f.webkitRelativePath || f.name,
    size: f.size,
    read: async () => new Uint8Array(await f.arrayBuffer()),
  }));
}

/**
 * (P5-SKILLS-3, skill-4) **Upload a skill folder**: pick it, see what will be created (or, per file, why it can't be),
 * then create the skill and its files.
 */
export function UploadSkillDialog({
  open,
  onOpenChange,
  scope,
  taken,
  owner,
  onCreated,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  scope: SkillScope;
  taken: string[];
  /** Whose skills these are: the agent, the workspace or the organization. */
  owner: string;
  onCreated: (id: string) => void;
}) {
  const create = useCreateSkillWithFiles(scope);
  const [result, setResult] = useState<FolderResult | null>(null);
  const [reading, setReading] = useState(false);
  const [readError, setReadError] = useState<unknown>(null);
  const picker = useRef<HTMLInputElement>(null);
  // Not in React's attribute list: set on the element.
  const setPicker = (el: HTMLInputElement | null) => {
    picker.current = el;
    el?.setAttribute("webkitdirectory", "");
  };
  // Closing forgets what was picked.
  const change = (next: boolean) => {
    if (!next) {
      setResult(null);
      setReadError(null);
      create.reset();
    }
    onOpenChange(next);
  };

  async function read(list: FileList) {
    setReading(true);
    setReadError(null);
    create.reset();
    try {
      setResult(await readSkillFolder(pickedFiles(list), taken, owner));
    } catch (error) {
      setReadError(error);
    } finally {
      setReading(false);
    }
  }

  const folder = result?.ok ? result.folder : null;
  const failed = create.error;
  return (
    <Dialog open={open} onOpenChange={change}>
      <DialogContent
        title="Upload a skill folder"
        description="A folder with a SKILL.md at its top level: a name and a description in its front matter, then the instructions."
        size="md"
        footer={
          <div className="flex flex-wrap justify-end gap-2">
            <DialogClose asChild>
              <Button type="button" variant="outline">
                Cancel
              </Button>
            </DialogClose>
            <Button
              type="button"
              disabled={!folder || create.isPending}
              onClick={() => {
                if (!folder) return;
                create.mutate(
                  {
                    skill: { name: folder.name, title: folder.title, description: folder.description, body: folder.body, enabled: true },
                    files: folder.files.map(({ path, content, executable }) => ({ path, content, executable })),
                  },
                  {
                    onSuccess: (made) => {
                      change(false);
                      onCreated(made.id);
                    },
                  },
                );
              }}
            >
              Create skill
            </Button>
          </div>
        }
      >
        <div className="space-y-3">
          <Button type="button" variant="outline" onClick={() => picker.current?.click()}>
            {result ? "Choose another folder" : "Choose a folder"}
          </Button>
          <input
            ref={setPicker}
            type="file"
            multiple
            className="sr-only"
            tabIndex={-1}
            aria-hidden="true"
            data-testid="skill-folder-input"
            onChange={(e) => {
              if (e.target.files?.length) void read(e.target.files);
              e.target.value = "";
            }}
          />
          {reading ? <Loading label="Reading the folder…" /> : null}
          <ErrorNotice error={readError} />
          {result && !result.ok ? (
            <Notice tone="danger" role="alert">
              <p className="font-medium">This folder can’t be added yet. Nothing was saved.</p>
              <ul className="mt-1 list-disc space-y-1 pl-5">
                {result.problems.map((p) => (
                  <li key={p} className="[overflow-wrap:anywhere]">
                    {p}
                  </li>
                ))}
              </ul>
            </Notice>
          ) : null}
          {folder ? (
            <section aria-label="What will be created" className="space-y-2">
              <p className="break-words text-sm font-semibold text-ink">{skillDisplayName(folder.name, folder.title)}</p>
              <p className="break-words text-sm text-ink-2">{folder.description}</p>
              <p className="text-xs text-muted">
                Folder: <code className="break-all font-mono">{folder.name}</code> · {folder.files.length + 1} {folder.files.length ? "files" : "file"} · {sizeWords(folder.size)}
              </p>
              <ul aria-label="Files to be created" className="divide-y divide-line rounded-control border border-line">
                {[{ path: "SKILL.md", size: [...folder.body].length, executable: false }, ...folder.files].map((f) => (
                  <li key={f.path} className="flex flex-wrap items-center gap-2 px-3 py-2">
                    <span className="min-w-0 flex-1 break-all font-mono text-xs text-ink">{f.path}</span>
                    {f.executable ? <ExecutableTag /> : null}
                    <span className="text-xs text-muted">{sizeWords(f.size)}</span>
                  </li>
                ))}
              </ul>
              {folder.skipped.length ? <p className="break-words text-xs text-muted">Left out: {folder.skipped.join(", ")}.</p> : null}
            </section>
          ) : null}
          {failed ? (
            <ErrorNotice
              error={failed}
              format={(e) => (e instanceof SkillFileError ? `“${e.path}” couldn’t be saved, so no skill was created: ${errorMessage(e.reason)}` : errorMessage(e))}
            />
          ) : null}
        </div>
      </DialogContent>
    </Dialog>
  );
}
