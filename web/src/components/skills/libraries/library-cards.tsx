"use client";

import { useState } from "react";

import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { CardGrid } from "@/components/ui/card-grid";
import { ConfirmDialog } from "@/components/ui/confirm-dialog";
import { ExternalLinkIcon, MoreIcon } from "@/components/ui/icons";
import { Menu, MenuContent, MenuItem, MenuSeparator, MenuTrigger } from "@/components/ui/menu";
import { ErrorNotice } from "@/components/ui/notice";
import { When } from "@/components/ui/when";
import {
  shortSha,
  useCheckLibraryNow,
  useDeleteSkillLibrary,
  usePatchSkillLibrary,
  type LibraryOwner,
  type SkillLibrary,
} from "@/lib/api/skill-libraries";
import type { Skill } from "@/lib/api/skills";

import { EditLibraryDialog } from "./edit-library-dialog";
import { LibraryStateChip, libraryReason, needsAddress } from "./library-status";

const total = (l: SkillLibrary) => (l.skill_counts ? l.skill_counts.agent + l.skill_counts.workspace + l.skill_counts.org : 0);

/** The branch or tag the library follows (the default one when none is set, if known); a commit id is shortened. */
function branchName(l: SkillLibrary): string | null {
  const name = l.ref || l.default_branch || null;
  return name && /^[0-9a-f]{40}$/i.test(name) ? shortSha(name) : name;
}

const skillsWord =(n: number) => `${n} ${n === 1 ? "skill" : "skills"}`;

/** How many skills the library offered at its last check (C-1176), when the server says. */
function offered(l: SkillLibrary): number | null {
  return l.skills_offered ?? null;
}

/** "17 skills · 1 used by Acme", "5 skills · none used yet"; the instance counts organizations instead. */
function countWords(l: SkillLibrary, orgName: string | undefined): string {
  const n = offered(l);
  const used = orgName === undefined ? (l.orgs_using ?? 0) : total(l);
  const by = orgName === undefined ? `used by ${used} ${used === 1 ? "organization" : "organizations"}` : `${used} used by ${orgName}`;
  const words = used === 0 ? "none used yet" : by;
  if (n === null) return words.charAt(0).toUpperCase() + words.slice(1);
  return `${skillsWord(n)} · ${words}`;
}

/** "Pinned e4bb5d9" when the organization's skills from the library share one commit, else "Pinned at 2 commits". */
function pinnedCommits(l: SkillLibrary, orgSkills: Skill[]): string[] {
  return [...new Set(orgSkills.flatMap((s) => (s.source && !s.source.detached && s.source.library_id === l.id && s.source.commit_sha ? [s.source.commit_sha] : [])))];
}

/**
 * The libraries of one level as cards (D-450, D-506, C-1129; skill-6): the organization's own, the instance's own
 * (`orgName` undefined) or the instance libraries an organization sees (`canManage` false: read-only, with **Browse
 * skills**). `orgId` is the organization whose list changes (`null` for the instance's own). One card per row up to a
 * content width of 896 px, as drawn; wider screens add columns.
 */
export function LibraryCards({
  orgId,
  orgName,
  rows,
  canManage,
  label,
  orgSkills = [],
  reviewHref,
  onBrowse,
}: {
  orgId: LibraryOwner;
  /** The organization whose use is counted; leave it out on the instance's own page. */
  orgName?: string;
  rows: SkillLibrary[];
  canManage: boolean;
  /** The accessible name of the list. */
  label: string;
  /** The organization-level skills, for the "Pinned" commit. */
  orgSkills?: Skill[];
  /** Where waiting updates are reviewed. */
  reviewHref?: string;
  /** Opens the library's skills read-only; absent where browsing is not possible (the instance's own page). */
  onBrowse?: (library: SkillLibrary) => void;
}) {
  const patch = usePatchSkillLibrary(orgId);
  const check = useCheckLibraryNow(orgId);
  const del = useDeleteSkillLibrary(orgId);
  const [removing, setRemoving] = useState<SkillLibrary | null>(null);
  const [editingId, setEditingId] = useState<string | null>(null);
  const editing = rows.find((r) => r.id === editingId) ?? null;
  const failed = patch.error ?? check.error ?? del.error;
  const instance = orgName === undefined;
  return (
    <div className="space-y-3">
      <ErrorNotice error={failed} />
      <CardGrid aria-label={label} className="@2xl:grid-cols-1 @4xl:grid-cols-2 @7xl:grid-cols-3">
        {rows.map((l) => {
          const paused = l.status === "paused";
          const reason = libraryReason(l);
          const pins = pinnedCommits(l, orgSkills);
          const checkId = `library-${l.id}-check`;
          const editId = `library-${l.id}-edit`;
          // What acts on the chip: the review link for an update, else Set address / Edit, else Check now.
          const actionId = (l.pending_updates ?? 0) > 0 && l.status === "active" ? `library-${l.id}-review` : canManage ? editId : undefined;
          return (
            <li key={l.id} className="min-w-0">
              <Card className="flex h-full flex-col gap-3">
                <div className="flex flex-wrap items-center gap-3">
                  <span aria-hidden="true" className="inline-flex size-9 shrink-0 items-center justify-center rounded-control bg-avatar-blue text-avatar-blue-ink">
                    <ExternalLinkIcon className="size-4" />
                  </span>
                  <div className="min-w-40 flex-1">
                    <h3 className="text-sm font-semibold text-ink [overflow-wrap:anywhere]">{l.label}</h3>
                    {/* The repository is repository text: plain text, never a link. */}
                    {l.repo ? <p className="font-mono text-xs text-muted [overflow-wrap:anywhere]">{l.repo}</p> : null}
                  </div>
                  <div className="ml-auto flex items-center gap-2">
                    <LibraryStateChip library={l} actionId={actionId} reviewHref={reviewHref} />
                    {canManage ? (
                      <Menu>
                        <MenuTrigger asChild>
                          <Button type="button" size="sm" variant="outline" aria-label={`More actions for ${l.label}`}>
                            <MoreIcon className="size-4" />
                          </Button>
                        </MenuTrigger>
                        <MenuContent align="end">
                          {onBrowse ? (
                            <MenuItem disabled={l.status !== "active"} onSelect={() => onBrowse(l)}>
                              Browse skills
                            </MenuItem>
                          ) : null}
                          <MenuItem disabled={patch.isPending} onSelect={() => patch.mutate({ id: l.id, body: { status: paused ? "active" : "paused" } })}>
                            {paused ? "Resume" : "Pause"}
                          </MenuItem>
                          <MenuSeparator />
                          <MenuItem className="text-danger-ink" onSelect={() => setRemoving(l)}>
                            Delete…
                          </MenuItem>
                        </MenuContent>
                      </Menu>
                    ) : null}
                  </div>
                </div>
                <ul className="flex flex-wrap gap-x-4 gap-y-1 text-sm text-ink-2">
                  <li>
                    {branchName(l) ? (
                      <>
                        Branch <b className="font-mono text-xs font-semibold">{branchName(l)}</b>
                      </>
                    ) : (
                      "Default branch"
                    )}
                  </li>
                  <li>
                    {l.last_refresh_at ? (
                      <>
                        Checked <When value={l.last_refresh_at} />
                      </>
                    ) : (
                      "Not checked yet"
                    )}
                  </li>
                  <li>{countWords(l, orgName)}</li>
                  {pins.length === 1 ? (
                    <li>
                      Pinned <b className="font-mono text-xs font-semibold">{shortSha(pins[0])}</b>
                    </li>
                  ) : pins.length > 1 ? (
                    <li>Pinned at {pins.length} commits</li>
                  ) : null}
                </ul>
                {reason ? <p className="text-sm text-danger-ink [overflow-wrap:anywhere]">{reason}</p> : null}
                <div className="mt-auto flex flex-wrap gap-2">
                  {canManage ? (
                    <>
                      {needsAddress(l) ? null : (
                        <Button id={checkId} size="sm" variant="outline" disabled={check.isPending || paused} onClick={() => check.mutate(l.id)} aria-label={`Check now ${l.label}`}>
                          Check now
                        </Button>
                      )}
                      {needsAddress(l) ? (
                        <Button id={editId} size="sm" variant="outline" onClick={() => setEditingId(l.id)} aria-label={`Set address of ${l.label}`}>
                          Set address
                        </Button>
                      ) : (
                        <Button id={editId} size="sm" variant="ghost" onClick={() => setEditingId(l.id)} aria-label={`Edit ${l.label}`}>
                          Edit
                        </Button>
                      )}
                    </>
                  ) : onBrowse && l.status === "active" ? (
                    <Button size="sm" variant="outline" onClick={() => onBrowse(l)} aria-label={`Browse skills of ${l.label}`}>
                      Browse skills
                    </Button>
                  ) : null}
                </div>
              </Card>
            </li>
          );
        })}
      </CardGrid>
      <EditLibraryDialog library={editing} orgId={orgId} open={!!editing} onOpenChange={(o) => !o && setEditingId(null)} />
      <ConfirmDialog
        open={!!removing}
        onOpenChange={(o) => !o && setRemoving(null)}
        title={`Delete ${removing?.label ?? "library"}?`}
        description={
          instance
            ? `${removing ? total(removing) : 0} skills added from it, in every organization, keep their content and become editable skills.`
            : `${removing ? total(removing) : 0} skills keep their content and become editable skills.`
        }
        confirmLabel="Delete library"
        danger
        busy={del.isPending}
        onConfirm={() => removing && del.mutate(removing.id, { onSuccess: () => setRemoving(null) })}
      />
    </div>
  );
}
