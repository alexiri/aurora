"use client";

import { useQueryClient } from "@tanstack/react-query";
import { useId, useState } from "react";

import { EmptyState, ErrorState, Loading } from "@/components/states";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Dialog, DialogClose, DialogContent } from "@/components/ui/dialog";
import { Field, inputClass } from "@/components/ui/field";
import { ChevronDownIcon, ChevronRightIcon, PlusIcon, SearchIcon } from "@/components/ui/icons";
import { SubScreenHeader } from "@/components/ui/list-detail";
import { Markdown } from "@/components/ui/markdown";
import { ErrorNotice, Notice } from "@/components/ui/notice";
import { Select } from "@/components/ui/select";
import { TextViewer } from "@/components/ui/text-viewer";
import { useMe } from "@/lib/api/queries";
import {
  libraryKeys,
  shortSha,
  useLibraryCatalogue,
  useLibrarySkill,
  useSkillLibraries,
  type BrowseFor,
  type LibraryBudget,
  type LibrarySkillContent,
  type LibrarySkillItem,
  type SkillLibrary,
} from "@/lib/api/skill-libraries";
import { useCreateSkill, type SkillScope } from "@/lib/api/skills";
import { formatCount } from "@/lib/format-date";
import { descriptionSentence, skillDisplayName } from "@/lib/skill-name";
import { useMediaQuery } from "@/lib/use-media-query";
import { cn } from "@/lib/utils";

import { ExecutableTag, SKILL_MD } from "./files";
import { AddLibraryDialog } from "./libraries/add-library-dialog";

/** Below this width the libraries are a select above the list and a skill's preview is its own screen. */
export const NARROW_BROWSER = "(max-width: 767px)";

/** The libraries a person can add from: the organization's own, then the ones the instance shares, active only (C-1128). */
export function activeLibraries(items: SkillLibrary[] | undefined): { own: SkillLibrary[]; shared: SkillLibrary[] } {
  const active = (items ?? []).filter((l) => l.status === "active");
  return { own: active.filter((l) => l.scope !== "instance"), shared: active.filter((l) => l.scope === "instance") };
}

/** Whether there is any library to add from: the organization's or one the instance shares, active. */
export function useCanAddFromLibrary(orgId: string | undefined): boolean {
  const groups = activeLibraries(useSkillLibraries(orgId).data?.items);
  return !!orgId && groups.own.length + groups.shared.length > 0;
}

/** Where a skill from a library is added. */
export interface AddTarget {
  scope: SkillScope;
  /** The agent's, workspace's or organization's name, as the page calls it. */
  subject: string;
  /** The ids of the skills already kept there, to tell "added here" from "added somewhere else". */
  skillIds: Set<string>;
  onAdded: (id: string) => void;
}

const browseFor = (scope: SkillScope): BrowseFor =>
  scope.kind === "agent" ? { tier: "agent", agent: scope.id } : scope.kind === "workspace" ? { tier: "workspace", workspace: scope.id } : { tier: "organization" };

// ---- words --------------------------------------------------------------------------------------

const kilobytes = (chars: number) => `${Math.max(1, Math.round(chars / 1024))} KB`;
const fileKilobytes = (chars: number) => `${Math.max(0.1, chars / 1024).toFixed(1)} KB`;
const filesWords = (n: number) => `${n} ${n === 1 ? "file" : "files"}`;
const skillsWords = (n: number) => `${n} ${n === 1 ? "skill" : "skills"}`;

const tooLarge = (subject: string, budget: LibraryBudget) => `It would exceed ${subject}’s ${formatCount(Math.round(budget.bytes / 1024))} KB of library skills.`;

/** Whether this very skill of this library is already kept where it would be added. */
export function addedHere(item: LibrarySkillItem, target: AddTarget | undefined): boolean {
  return target ? item.added.some((a) => target.skillIds.has(a.id)) : false;
}

/** Why a listed skill can't be added, in plain words; null when it can (or is the one already added). */
export function blockedWords(item: LibrarySkillItem, subject: string, budget: LibraryBudget | null | undefined): string | null {
  if (!item.valid) return item.reason ?? item.blocked?.message ?? "This skill can't be added.";
  const blocked = item.blocked;
  if (!blocked) return null;
  if (blocked.reason === "name_taken") return `A skill named ${item.name} is already on ${subject}.`;
  // The budget has a size and a number of skills; the server's own sentence says which when it is not the size.
  if (blocked.reason === "budget" && budget && budget.used + item.size > budget.bytes) return tooLarge(subject, budget);
  return blocked.message;
}

/** The list as it is drawn: what can be picked (what is added here stays in it, marked) and what can't be added. */
export function splitCatalogue(items: LibrarySkillItem[], target: AddTarget | undefined, budget: LibraryBudget | null | undefined) {
  const listed: { item: LibrarySkillItem; added: boolean }[] = [];
  const folded: { item: LibrarySkillItem; why: string }[] = [];
  for (const item of items) {
    // Without a target (looking at a library) "Added" means added anywhere the viewer can see.
    const added = target ? addedHere(item, target) : item.added.length > 0;
    const why = added && item.valid ? null : blockedWords(item, target?.subject ?? "", budget);
    if (why) folded.push({ item, why });
    else listed.push({ item, added });
  }
  return { listed, folded };
}

// ---- the preview --------------------------------------------------------------------------------

/** Exactly what an add would keep, readable: the description, the instructions rendered, and each file. */
function Preview({ item, content, commit }: { item: LibrarySkillItem; content: LibrarySkillContent; commit: string | undefined }) {
  const [open, setOpen] = useState<string | null>(null);
  const body = content.body ?? "";
  const files = [{ path: SKILL_MD, size: [...body].length, executable: false, binary: false, content: body }, ...content.files];
  const shown = files.find((f) => f.path === open);
  return (
    <div className="min-w-0 space-y-4" aria-label="Preview of what will be stored" role="group">
      {/* Repository text stays as its author wrote it. */}
      <p className="break-words border-b border-line pb-4 text-sm text-ink-2">{descriptionSentence(content.description ?? item.description ?? "")}</p>
      <section aria-label="Instructions" className="min-w-0 [overflow-wrap:anywhere]">
        {/* An outsider's text: showing it fetches nothing. */}
        <Markdown source={body} images={false} />
      </section>
      <ul aria-label="Files" className="divide-y divide-line overflow-hidden rounded-control border border-line">
        {files.map((f) => (
          <li key={f.path}>
            <button type="button" aria-expanded={open === f.path} className={cn("flex min-h-10 w-full min-w-0 items-center gap-2 px-3 py-2 text-left hover:bg-subtle", open === f.path && "bg-subtle")} onClick={() => setOpen(open === f.path ? null : f.path)}>
              <span className="min-w-0 flex-1 break-all font-mono text-xs text-ink">{f.path}</span>
              {f.executable ? <ExecutableTag /> : null}
              <span className="shrink-0 text-xs text-muted">{fileKilobytes(f.size)}</span>
            </button>
          </li>
        ))}
      </ul>
      {shown ? (
        shown.binary ? (
          <p className="text-sm text-muted">This file isn&apos;t text, so it isn&apos;t shown.</p>
        ) : (
          <TextViewer label={`File ${shown.path}`} text={shown.content ?? ""} className="whitespace-pre-wrap [overflow-wrap:anywhere]" />
        )
      ) : null}
      <p className="text-xs text-muted">
        Pinned to <span className="font-mono">{shortSha(commit)}</span> when added.
      </p>
    </div>
  );
}

// ---- the browser --------------------------------------------------------------------------------

const selectedRow = "row-rail block w-full min-w-0 border-l-transparent px-4 py-2.5 text-left hover:bg-subtle aria-[current=true]:border-l-accent aria-[current=true]:bg-subtle";

function LibraryChoice({ library, current, onChoose }: { library: SkillLibrary; current: boolean; onChoose: () => void }) {
  return (
    <li>
      <button type="button" aria-current={current} className={selectedRow} onClick={onChoose}>
        <span className="block break-words text-sm font-semibold text-ink">{library.label}</span>
        {typeof library.skills_offered === "number" ? <span className="block text-xs text-muted">{skillsWords(library.skills_offered)}</span> : null}
      </button>
    </li>
  );
}

/**
 * (P5-SKILLS-4, skill-5; D-496, C-1176) The libraries on the left, the skills of one that can be added on the right with
 * those that can't folded away, and the chosen one in full before it is added. With a `target` it ends in
 * **Add to <name>**; without one (an organization looking at a library) it only reads. Library labels and everything
 * from a repository are data: plain text, and instructions that fetch nothing.
 */
export function LibraryBrowserDialog({
  orgId,
  groups,
  open,
  onOpenChange,
  initialId,
  target,
}: {
  orgId: string;
  groups: { own: SkillLibrary[]; shared: SkillLibrary[] };
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** The library to show first (Browse on a card). */
  initialId?: string;
  target?: AddTarget;
}) {
  return open ? <BrowserFlow orgId={orgId} groups={groups} onOpenChange={onOpenChange} initialId={initialId} target={target} /> : null;
}

/** Mounted only while open, so every visit starts afresh. */
function BrowserFlow({
  orgId,
  groups,
  onOpenChange,
  initialId,
  target,
}: {
  orgId: string;
  groups: { own: SkillLibrary[]; shared: SkillLibrary[] };
  onOpenChange: (open: boolean) => void;
  initialId?: string;
  target?: AddTarget;
}) {
  const narrow = useMediaQuery(NARROW_BROWSER);
  const searchId = useId();
  const pickId = useId();
  const whyId = useId();
  const reasonId = useId();
  const all = [...groups.own, ...groups.shared];
  const [libId, setLibId] = useState<string>(initialId ?? "");
  const [q, setQ] = useState("");
  const [picked, setPicked] = useState<string | null>(null);
  const [why, setWhy] = useState(false);
  const [adding, setAdding] = useState(false);
  const current = (all.some((l) => l.id === libId) ? libId : "") || initialId || all[0]?.id;

  // (P5-SKLIB-2, C-1128) A library shared by the instance is browsed as this organization; (C-1176) naming where the
  // skill would go gets each one's `blocked` and the room left there.
  const catalogue = useLibraryCatalogue(current, q, orgId, target ? browseFor(target.scope) : undefined);
  // Every page is read at one commit; the first page names it. `truncated` (past 200) is on the last page.
  const head = catalogue.data?.pages[0];
  const lastPage = catalogue.data?.pages[catalogue.data.pages.length - 1];
  const items = (catalogue.data?.pages ?? []).flatMap((p) => p.items);
  const budget = head?.budget;
  const { listed, folded } = splitCatalogue(items, target, budget);
  // Beside the list there is always a skill to read; on a narrow screen only the one that was pressed.
  const shown = items.find((i) => i.path === picked) ?? (narrow ? null : (listed.find((r) => !r.added)?.item ?? listed[0]?.item ?? null));
  const content = useLibrarySkill(current, shown?.path, head?.commit_sha, orgId);

  const me = useMe().data;
  const membership = me?.memberships?.find((m) => (m.org as { id?: string }).id === orgId);
  const role = (membership as { effective_role?: string | null } | undefined)?.effective_role ?? membership?.role;
  const mayAddLibrary = !!target && (role === "owner" || role === "admin");

  const qc = useQueryClient();
  const create = useCreateSkill(target?.scope ?? { kind: "organization", id: orgId });

  const choose = (path: string | null) => {
    create.reset();
    setPicked(path);
  };
  const chooseLibrary = (id: string) => {
    choose(null);
    setWhy(false);
    setQ("");
    setLibId(id);
  };

  // Why the shown skill can't be added; the read of its files can find more than the listing's estimate did.
  const blockedNow = (() => {
    if (!target || !shown) return null;
    if (addedHere(shown, target)) return `Already added to ${target.subject}.`;
    const listedWhy = blockedWords(shown, target.subject, budget);
    if (listedWhy) return listedWhy;
    if (content.data && !content.data.valid) return content.data.reason ?? "This skill can't be added.";
    if (content.data && budget && budget.used + content.data.size > budget.bytes) return tooLarge(target.subject, budget);
    return null;
  })();

  async function add() {
    if (!target || !shown || !head || !current) return;
    try {
      const made = await create.mutateAsync({ from_library: { library_id: current, path: shown.path, commit_sha: head.commit_sha } });
      void qc.invalidateQueries({ queryKey: libraryKeys.one(current) });
      onOpenChange(false);
      target.onAdded(made.id);
    } catch {
      // Said on the preview; the list is read again, since what can be added may have changed.
      void qc.invalidateQueries({ queryKey: libraryKeys.one(current) });
    }
  }

  const list = (
    <div className="flex min-h-0 min-w-0 flex-1 flex-col">
      <div className="space-y-3 px-4 py-3">
        {narrow ? (
          <Field id={pickId} label="Library">
            <Select id={pickId} value={current} onChange={chooseLibrary}>
              {groups.own.length > 0 ? (
                <optgroup label="Your organization">
                  {groups.own.map((l) => (
                    <option key={l.id} value={l.id}>
                      {l.label}
                    </option>
                  ))}
                </optgroup>
              ) : null}
              {groups.shared.length > 0 ? (
                <optgroup label="Shared with every organization">
                  {groups.shared.map((l) => (
                    <option key={l.id} value={l.id}>
                      {l.label}
                    </option>
                  ))}
                </optgroup>
              ) : null}
            </Select>
          </Field>
        ) : null}
        {narrow && mayAddLibrary ? (
          <Button type="button" variant="ghost" size="sm" className="min-h-11 text-accent-ink" onClick={() => setAdding(true)}>
            <PlusIcon className="size-4" />
            Add a library
          </Button>
        ) : null}
        <div className="relative">
          <label htmlFor={searchId} className="sr-only">
            Search skills
          </label>
          <SearchIcon className="pointer-events-none absolute left-3 top-1/2 size-4 -translate-y-1/2 text-muted" />
          <input id={searchId} type="search" placeholder="Search skills" autoComplete="off" className={cn(inputClass, "min-h-11 pl-9")} value={q} onChange={(e) => setQ(e.target.value)} />
        </div>
      </div>
      {catalogue.isPending ? (
        <div className="px-4">
          <Loading label="Reading the repository…" />
        </div>
      ) : null}
      {catalogue.isError && !head ? (
        <div className="px-4">
          <ErrorState error={catalogue.error} onRetry={() => catalogue.refetch()} />
        </div>
      ) : null}
      {head ? (
        // A region of its own that scrolls, so it can be reached by keyboard.
        <div tabIndex={0} role="region" aria-label="Skills in this library" className="min-h-0 flex-1 overflow-y-auto border-t border-line">
          {lastPage?.truncated ? (
            <Notice role="status" className="m-4">
              This repository has more skills than can be listed. Ask an admin to choose a narrower folder for this library.
            </Notice>
          ) : null}
          {items.length === 0 ? (
            <div className="p-4">
              <EmptyState title="No skills found" />
            </div>
          ) : null}
          <ul aria-label="Library skills" className="divide-y divide-line">
            {listed.map(({ item, added }) => (
              <li key={item.path}>
                <button type="button" aria-current={shown?.path === item.path} className={cn(selectedRow, "[overflow-wrap:anywhere]")} onClick={() => choose(item.path)}>
                  <span className="flex items-start justify-between gap-2">
                    <span className="min-w-0">
                      <span className={cn("block text-sm font-semibold", added ? "text-muted" : "text-ink")}>{skillDisplayName(item.name)}</span>
                      <span className={cn("line-clamp-1 text-xs", added ? "text-muted" : "text-ink-2")}>{descriptionSentence(item.description ?? "")}</span>
                      <span className="block text-xs text-muted">
                        {filesWords(item.files)} · {kilobytes(item.size)}
                      </span>
                    </span>
                    {added ? (
                      <Badge tone="accent" className="shrink-0 self-center text-xs">
                        Added
                      </Badge>
                    ) : null}
                  </span>
                </button>
              </li>
            ))}
          </ul>
          {catalogue.hasNextPage ? (
            <div className="px-4 py-3">
              <Button type="button" variant="outline" size="sm" disabled={catalogue.isFetchingNextPage} onClick={() => void catalogue.fetchNextPage()}>
                {catalogue.isFetchingNextPage ? "Loading…" : "Load more"}
              </Button>
            </div>
          ) : null}
          {catalogue.isFetchNextPageError ? <ErrorState error={catalogue.error} onRetry={() => void catalogue.fetchNextPage()} /> : null}
        </div>
      ) : null}
      {/* Pinned to the foot of the column; opened, the reasons scroll in their own part of it. */}
      {head && folded.length > 0 ? (
        <div className={cn("flex min-h-0 shrink-0 flex-col border-t border-line", why && "h-1/2")}>
          <button type="button" aria-expanded={why} aria-controls={whyId} className="flex min-h-11 w-full shrink-0 items-center gap-2 px-4 py-2 text-left text-sm text-ink-2 hover:bg-subtle" onClick={() => setWhy(!why)}>
            {why ? <ChevronDownIcon className="size-4 shrink-0" /> : <ChevronRightIcon className="size-4 shrink-0" />}
            <span>
              {folded.length} can’t be added · <span className="text-link underline">Why?</span>
            </span>
          </button>
          <div id={whyId} hidden={!why} tabIndex={0} role="region" aria-label="Why they can’t be added" className="min-h-0 flex-1 overflow-y-auto border-t border-line">
            <ul aria-label="Skills that can’t be added" className="divide-y divide-line">
              {folded.map(({ item, why: reason }) => (
                <li key={item.path} className="px-4 py-2 [overflow-wrap:anywhere]">
                  <span className="block text-sm font-medium text-ink-2">{skillDisplayName(item.name)}</span>
                  <span className="block text-xs text-muted">{reason}</span>
                </li>
              ))}
            </ul>
          </div>
        </div>
      ) : null}
    </div>
  );

  const preview = shown ? (
    <div tabIndex={0} role="region" aria-label={skillDisplayName(shown.name)} className="min-h-0 min-w-0 flex-1 space-y-3 overflow-y-auto px-5 py-3">
      {narrow ? <SubScreenHeader backLabel="Skills" onBack={() => choose(null)} /> : null}
      <h3 className="break-words font-display text-lg font-semibold text-ink">{skillDisplayName(shown.name)}</h3>
      <ErrorNotice error={create.error} />
      {blockedNow ? (
        <Notice id={reasonId} role="status">
          {blockedNow}
        </Notice>
      ) : null}
      {content.isPending ? <Loading /> : null}
      {content.isError ? <ErrorState error={content.error} onRetry={() => content.refetch()} /> : null}
      {content.data?.valid ? <Preview key={`${current}/${shown.path}`} item={shown} content={content.data} commit={head?.commit_sha} /> : null}
    </div>
  ) : (
    <div />
  );

  const showAdd = !!target && (!narrow || !!shown);
  const footer = (
    <div className="flex flex-wrap items-center justify-end gap-x-4 gap-y-2">
      {target ? (
        <p className="min-w-0 flex-1 basis-60 text-sm text-ink-2">
          {target.scope.kind === "agent" ? `Adds a copy to ${target.subject}.` : `Adds it to ${target.subject}’s skills.`} You can turn it off or remove it later.
        </p>
      ) : null}
      <div className="flex items-center gap-2">
        <DialogClose asChild>
          <Button type="button" variant={target ? "ghost" : "outline"}>
            {target ? "Cancel" : "Close"}
          </Button>
        </DialogClose>
        {showAdd ? (
          <Button type="button" variant="approve" aria-describedby={blockedNow ? reasonId : undefined} disabled={!shown || !content.data?.valid || !!blockedNow || create.isPending} onClick={() => void add()}>
            Add to {target.subject}
          </Button>
        ) : null}
      </div>
    </div>
  );

  return (
    <Dialog open onOpenChange={onOpenChange}>
      <DialogContent
        title={target ? "Add a skill from a library" : "Browse a library"}
        description={target ? "Skills are pinned to the version you see here." : "What this library offers at its current version. Add skills from an agent’s, a workspace’s or the organization’s Skills page."}
        size="2xl"
        variant="sheet"
        // As tall as the mockup; the dialog's own cap keeps it inside a shorter window.
        fixedHeight="760px"
        className="max-sm:max-w-none"
        bodyClassName="flex flex-col overflow-hidden border-t border-line p-0"
        footer={footer}
      >
        {narrow ? (
          <div className="flex min-h-0 flex-1 flex-col">{shown ? preview : list}</div>
        ) : (
          <div className="flex min-h-0 flex-1 divide-x divide-line">
            <nav aria-label="Libraries" className="min-h-0 w-44 shrink-0 overflow-y-auto py-2">
              <p className="px-4 pb-1 text-label font-semibold uppercase tracking-label text-muted">Libraries</p>
              <ul className="divide-y divide-line">
                {groups.own.map((l) => (
                  <LibraryChoice key={l.id} library={l} current={l.id === current} onChoose={() => chooseLibrary(l.id)} />
                ))}
              </ul>
              {groups.shared.length > 0 ? (
                <>
                  <p className="px-4 pb-1 pt-3 text-label font-semibold uppercase tracking-label text-muted">Shared with every organization</p>
                  <ul aria-label="Shared with every organization" className="divide-y divide-line">
                    {groups.shared.map((l) => (
                      <LibraryChoice key={l.id} library={l} current={l.id === current} onChoose={() => chooseLibrary(l.id)} />
                    ))}
                  </ul>
                </>
              ) : null}
              {mayAddLibrary ? (
                <button type="button" className="flex min-h-11 w-full items-center gap-1.5 px-4 text-left text-sm font-medium text-link" onClick={() => setAdding(true)}>
                  <PlusIcon className="size-4" />
                  Add a library
                </button>
              ) : null}
            </nav>
            <div className="flex min-h-0 min-w-0 flex-1 basis-0 flex-col">{list}</div>
            <div className="flex min-h-0 min-w-0 flex-1 basis-0 flex-col">{preview}</div>
          </div>
        )}
        <AddLibraryDialog orgId={orgId} open={adding} onOpenChange={setAdding} />
      </DialogContent>
    </Dialog>
  );
}

/** **Add from a library**, from a Skills page's Add skill menu. */
export function AddFromLibrary({ orgId, open, onOpenChange, target }: { orgId: string | undefined; open: boolean; onOpenChange: (open: boolean) => void; target: AddTarget }) {
  const libs = useSkillLibraries(orgId);
  const groups = activeLibraries(libs.data?.items);
  if (!orgId || groups.own.length + groups.shared.length === 0) return null;
  return <LibraryBrowserDialog orgId={orgId} groups={groups} open={open} onOpenChange={onOpenChange} target={target} />;
}
