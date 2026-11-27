"use client";

import Link from "next/link";
import { useId, useState, type ReactNode } from "react";

import { AttentionChip } from "@/components/ui/attention-chip";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { ConfirmDialog } from "@/components/ui/confirm-dialog";
import { ActivityIcon, LockIcon, MoreIcon } from "@/components/ui/icons";
import { SubScreenHeader } from "@/components/ui/list-detail";
import { Markdown } from "@/components/ui/markdown";
import { Menu, MenuContent, MenuItem, MenuTrigger } from "@/components/ui/menu";
import { ErrorNotice, Notice } from "@/components/ui/notice";
import { shortSha, useDetachLibrarySkill, useSkillLibraryInfo, type BudgetTier, type LibraryUpdate } from "@/lib/api/skill-libraries";
import { readSkillFile, useCreateSkillWithFiles, useDeleteSkill, type Skill, type SkillScope } from "@/lib/api/skills";
import { useItToLine } from "@/lib/skill-name";

import { SkillFileTree } from "./files";
import { UpdateReviewDialog } from "./library";
import { SkillSwitch, SourceChip } from "./list";
import { isLibrary, libraryLabel, managers, pageScope, pageSubject, pageTier, usageWords, type SkillPage, type SkillRow } from "./model";

/** What the viewer may do with a skill on this page. */
export function skillRights(page: SkillPage, row: SkillRow) {
  // Whether this page is where the skill is kept, and the viewer may write there.
  const here =
    page.kind === "agent" ? row.tier === "agent" && !page.readOnly : row.tier === pageTier(page) && (page.kind === "organization" || page.canOwn);
  const library = isLibrary(row.skill);
  const canDecide = row.tier === "app" ? false : page.kind === "agent" ? page.canDecide[row.tier] : here;
  return { here, library, canDecide, edit: here && !library };
}

/** The sentences of the lock box, for a skill with no fields on this page; null for one that can be edited here. */
function lockSentences(page: SkillPage, row: SkillRow): ReactNode[] | null {
  const rights = skillRights(page, row);
  if (rights.edit) return null;
  const out: ReactNode[] = [];
  if (row.tier === "app") return [`Comes with the ${row.app?.name ?? ""} app.`];
  if (page.kind === "agent") {
    if (row.tier !== "agent") {
      const by = `Managed by ${managers(row.tier, page.orgName)}.`;
      if (row.fixedWhy) out.push(`${by} They turned it off.`);
      else out.push(page.readOnly ? by : `${by} You can turn it off for ${page.agentName}.`);
    }
  } else if (!rights.here) out.push(`Managed by ${managers(row.tier, page.kind === "organization" ? page.label : "")}.`);
  if (rights.library) {
    const from = (
      <>
        From {libraryLabel(row.skill)} · pinned to <code className="font-mono text-xs">{shortSha(row.skill.source?.commit_sha)}</code>.
      </>
    );
    out.push(out.length ? from : <>{from} Changes come from the library, not from here.</>);
  }
  return out.length ? out : ["You can read this skill. You can’t change it."];
}

function changedFiles(update: LibraryUpdate): number | null {
  const d = update.diff;
  if (!d) return null;
  return (d.body.changed || d.description.changed ? 1 : 0) + d.files.filter((f) => f.change !== "unchanged").length;
}

/** An update of a library skill that waits, for the people who may approve it. */
function UpdateAvailable({ skill, tier }: { skill: Skill; tier: BudgetTier }) {
  const [open, setOpen] = useState(false);
  const buttonId = useId();
  const info = useSkillLibraryInfo(skill.id);
  const update = info.data?.update;
  const n = update ? changedFiles(update) : null;
  const what =
    update?.kind === "removed"
      ? "removed from the repository."
      : update?.kind === "invalid"
        ? "the new version can’t be used."
        : n !== null && n > 0
          ? `${n} ${n === 1 ? "file" : "files"} changed upstream.`
          : null;
  return (
    <div className="space-y-2 rounded-control border border-pending-line bg-pending-panel p-3" data-testid="update-available">
      <p className="text-sm text-pending-ink">
        <AttentionChip actionId={buttonId}>Update available</AttentionChip>
        {what ? ` — ${what}` : null}
      </p>
      <Button id={buttonId} type="button" variant="outline" size="sm" onClick={() => setOpen(true)}>
        Review update
      </Button>
      <UpdateReviewDialog skill={skill} tier={tier} canDecide open={open} onOpenChange={setOpen} />
    </div>
  );
}

/** Agents whose own enabled skill of the same name overrides a workspace skill (`14`). */
function OverriddenIn({ skill }: { skill: SkillRow["skill"] }) {
  const list = skill.overridden_in ?? [];
  if (skill.tier !== "workspace" || list.length === 0) return null;
  return (
    <Notice role="status">
      Overridden by an agent skill of the same name in:{" "}
      {list.map((o, i) => (
        <span key={o.skill_id}>
          {i > 0 ? ", " : ""}
          <span className="font-mono">{o.agent.handle}</span>
        </span>
      ))}
      . Those agents do not receive this skill.
    </Notice>
  );
}

/**
 * (P5-SKILLS-3, skill-2, skill-3) One skill, readable first: its name, source and switch, when it is used, the
 * instructions rendered, its files, and how often it was used. What cannot be changed here has no fields at all: a lock
 * line says who manages it.
 */
export function SkillPanel({
  page,
  row,
  taken,
  busy,
  narrow,
  onToggle,
  onEdit,
  onBack,
  onRemoved,
  onDuplicated,
}: {
  page: SkillPage;
  row: SkillRow;
  /** The folder names of the agent's own skills. */
  taken: string[];
  busy: boolean;
  narrow: boolean;
  onToggle: (row: SkillRow, on: boolean) => void;
  onEdit: () => void;
  onBack: () => void;
  onRemoved: () => void;
  onDuplicated: (id: string) => void;
}) {
  const skill = row.skill;
  const rights = skillRights(page, row);
  const subject = pageSubject(page);
  const scope: SkillScope = pageScope(page);
  const del = useDeleteSkill(scope);
  const detach = useDetachLibrarySkill(skill.id ?? "");
  const copy = useCreateSkillWithFiles(page.kind === "agent" ? { kind: "agent", id: page.agentId } : scope);
  const [confirm, setConfirm] = useState<"remove" | "stop" | null>(null);
  const [copying, setCopying] = useState(false);
  const [copyError, setCopyError] = useState<unknown>(null);
  const lock = lockSentences(page, row);
  const usage = usageWords(page, row);
  const asWritten = rights.library || row.tier === "app";

  // An app skill's files cannot be read, so one with files cannot be copied whole.
  const canDuplicate =
    page.kind === "agent" && !page.readOnly && row.tier !== "agent" && (!!skill.id || skill.files.length === 0) && !taken.includes(skill.name);
  const canStop = rights.library && rights.canDecide && !!skill.id;
  const removeFrom = page.kind === "agent" ? page.agentName : page.kind === "workspace" ? "this workspace" : page.label;
  const tryDraft = page.kind === "agent" && page.chat && row.on && !page.readOnly ? page.chat(`Use the “${row.name}” skill to `) : null;

  async function duplicate() {
    setCopying(true);
    setCopyError(null);
    try {
      const files = await Promise.all(
        skill.files.map(async (f) => ({ path: f.path, executable: f.executable, content: await readSkillFile(skill.id!, f.path) })),
      );
      const made = await copy.mutateAsync({
        skill: { name: skill.name, title: skill.title ?? null, description: skill.description, body: skill.body, enabled: true },
        files,
      });
      onDuplicated(made.id);
    } catch (error) {
      setCopyError(error);
    } finally {
      setCopying(false);
    }
  }

  const switchControl = <SkillSwitch page={page} row={row} busy={busy} onToggle={onToggle} />;
  const menu =
    canDuplicate || canStop || rights.here ? (
      <Menu>
        <MenuTrigger asChild>
          <Button type="button" variant="outline" size="sm" aria-label={`More actions for ${row.name}`} className="max-md:min-h-11">
            <MoreIcon className="size-4" />
          </Button>
        </MenuTrigger>
        <MenuContent>
          {canDuplicate ? (
            <MenuItem disabled={copying} onSelect={() => void duplicate()}>
              Duplicate as {subject}’s own
            </MenuItem>
          ) : null}
          {canStop ? <MenuItem onSelect={() => setConfirm("stop")}>Stop updates and edit</MenuItem> : null}
          {rights.here ? (
            <MenuItem className="text-danger-ink" onSelect={() => setConfirm("remove")}>
              Remove from {removeFrom}…
            </MenuItem>
          ) : null}
        </MenuContent>
      </Menu>
    ) : null;

  const content = (
    <article aria-label={row.name} className="min-w-0 space-y-4">
      <header className="space-y-3">
        {/* The switch keeps the top right corner; the name and its chip wrap beside it. */}
        <div className="flex items-start justify-between gap-3">
          <div className="flex min-w-0 flex-wrap items-center gap-x-2.5 gap-y-1">
            <h3 className="min-w-0 break-words font-display text-lg font-semibold text-ink">{row.name}</h3>
            <SourceChip page={page} row={row} />
          </div>
          <div className="shrink-0 pt-0.5">{switchControl}</div>
        </div>
        {lock ? (
          <div className="flex gap-2.5 rounded-control bg-subtle p-3 text-sm text-ink-2" data-testid="skill-lock">
            <LockIcon className="mt-0.5 size-4 shrink-0" aria-hidden="true" />
            <div className="min-w-0 space-y-1 break-words">
              {lock.map((line, i) => (
                <p key={i}>{line}</p>
              ))}
            </div>
          </div>
        ) : null}
        {rights.library && rights.canDecide && skill.source?.update_pending && skill.id && (row.tier === "agent" || row.tier === "workspace" || row.tier === "org") ? (
          <UpdateAvailable skill={skill as Skill} tier={row.tier} />
        ) : null}
        {skill.source?.detached ? (
          <p className="break-words text-xs text-muted">
            Was from {skill.source.repo} at {shortSha(skill.source.commit_sha)}. Updates are stopped.
          </p>
        ) : null}
        {page.kind === "workspace" ? <OverriddenIn skill={skill} /> : null}
        <p className="break-words text-sm text-ink-2">{useItToLine(skill.description, asWritten)}</p>
        {rights.edit || tryDraft || menu ? (
          <div className="flex flex-wrap items-center gap-2">
            {rights.edit ? (
              <Button type="button" variant="outline" size="sm" className="max-md:min-h-11" onClick={onEdit}>
                Edit
              </Button>
            ) : null}
            {tryDraft ? (
              <Button asChild size="sm" variant={rights.edit ? "approve" : "outline"} className="max-md:min-h-11">
                <Link href={tryDraft}>Try it in chat</Link>
              </Button>
            ) : null}
            {menu}
          </div>
        ) : null}
        <ErrorNotice error={del.error ?? detach.error ?? copyError} />
      </header>
      <section aria-label="Instructions" className="space-y-2 border-t border-line pt-4">
        <p className="text-label font-semibold uppercase tracking-label text-muted">Instructions</p>
        {skill.body.trim() ? <Markdown source={skill.body} /> : <p className="text-sm text-muted">No instructions.</p>}
      </section>
      <SkillFileTree key={skill.updated_at ?? ""} skillId={skill.id} version={skill.updated_at} body={skill.body} files={skill.files} />
      {usage ? (
        <p className="flex items-start gap-2 text-xs text-muted" data-testid="skill-usage">
          <ActivityIcon className="mt-0.5 size-3.5 shrink-0" aria-hidden="true" />
          <span className="min-w-0 break-words">{usage}</span>
        </p>
      ) : null}
      <ConfirmDialog
        open={confirm === "remove"}
        onOpenChange={(o) => setConfirm(o ? "remove" : null)}
        title={`Remove “${row.name}” from ${removeFrom}?`}
        description={
          rights.library
            ? "It stops being used. It can be added from the library again."
            : "It stops being used, and the skill and its files are deleted. This can't be undone."
        }
        confirmLabel="Remove skill"
        danger
        busy={del.isPending}
        onConfirm={() => {
          setConfirm(null);
          del.mutate(skill.id!, { onSuccess: onRemoved });
        }}
      />
      <ConfirmDialog
        open={confirm === "stop"}
        onOpenChange={(o) => setConfirm(o ? "stop" : null)}
        title={`Stop updates and edit “${row.name}”?`}
        description="It becomes an ordinary skill you can edit. The library no longer proposes updates to it."
        confirmLabel="Stop updates and edit"
        busy={detach.isPending}
        onConfirm={() => {
          setConfirm(null);
          detach.mutate(undefined, { onSuccess: onEdit });
        }}
      />
    </article>
  );

  return (
    <div className="min-w-0 space-y-3">
      {narrow ? <SubScreenHeader backLabel="Skills" onBack={onBack} /> : null}
      {narrow ? content : <Card>{content}</Card>}
    </div>
  );
}
