"use client";

import { useState } from "react";

import { EmptyState, ErrorState, Loading } from "@/components/states";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Dialog, DialogClose, DialogContent } from "@/components/ui/dialog";
import { inputClass } from "@/components/ui/field";
import { Notice } from "@/components/ui/notice";
import { ApiError } from "@/lib/api/client";
import { errorMessage } from "@/lib/api/errors";
import {
  LIBRARY_BUDGET,
  diffLines,
  shortSha,
  useApproveLibraryUpdate,
  useDetachLibrarySkill,
  useLibraryCatalogue,
  useLibrarySkill,
  usePendingLibraryUpdates,
  useRejectLibraryUpdate,
  useSkillLibraries,
  useSkillLibraryInfo,
  type BudgetTier,
  type LibraryDiff,
  type LibrarySkillContent,
  type LibrarySkillItem,
} from "@/lib/api/skill-libraries";
import { TIER_LABEL, useCreateSkill, type Skill, type SkillScope, type SkillTier } from "@/lib/api/skills";

type Source = NonNullable<Skill["source"]>;

/** Everything from a repository is untrusted text: it is only ever rendered as React text nodes (never as markup or links). */
export function LibraryBadge({ source }: { source: Source }) {
  if (source.detached) {
    return (
      <Badge>
        Detached · was {source.repo}@{shortSha(source.commit_sha)}
      </Badge>
    );
  }
  return (
    <>
      <Badge>
        Library · {source.repo} · {shortSha(source.commit_sha)}
      </Badge>
      {source.update_pending ? <Badge tone="pending">Update available</Badge> : null}
    </>
  );
}

/** "N updates are waiting": the pending proposals this scope's authority may decide. */
export function PendingUpdatesNotice({ scope, enabled }: { scope: SkillScope; enabled: boolean }) {
  const pending = usePendingLibraryUpdates(scope, enabled);
  const n = pending.data?.items.length ?? 0;
  if (n === 0) return null;
  return (
    <Notice tone="pending" role="status">
      {n === 1 ? "1 update from a skill library is waiting for review." : `${n} updates from skill libraries are waiting for review.`} Open a skill marked &quot;Update available&quot;.
    </Notice>
  );
}

/** Banner on a read-only library skill. */
export function LibraryManagedNotice() {
  return <Notice role="status">Managed by a skill library; edit it in the repository or detach it.</Notice>;
}

const AUTHORITY: Record<BudgetTier, string> = {
  agent: "use this agent",
  workspace: "own this workspace",
  org: "be an organization owner or admin",
};

// ---- the whole diff ---------------------------------------------------------------------------

function Lines({ old, next, label }: { old: string; next: string; label: string }) {
  const lines = diffLines(old, next);
  return (
    <pre tabIndex={0} aria-label={label} className="max-w-full overflow-x-auto rounded-control border border-line bg-canvas p-2 font-mono text-xs leading-5 text-ink">
      {lines.map((l, i) => (
        <span
          key={i}
          data-diff={l.kind}
          className={`block whitespace-pre ${l.kind === "add" ? "bg-accent-soft text-accent-ink" : l.kind === "del" ? "bg-danger-soft text-danger-ink" : ""}`}
        >
          {l.kind === "add" ? "+ " : l.kind === "del" ? "- " : "  "}
          {l.text}
        </span>
      ))}
    </pre>
  );
}

/** The whole stored-versus-proposed content: every line, nothing summarized or truncated. */
export function DiffView({ diff }: { diff: LibraryDiff }) {
  return (
    <div className="space-y-4">
      <section aria-label="Description change" className="space-y-1">
        <h4 className="text-sm font-semibold text-ink">Description{diff.description.changed ? "" : " (unchanged)"}</h4>
        <Lines old={diff.description.old} next={diff.description.new} label="Description diff" />
      </section>
      <section aria-label="SKILL.md change" className="space-y-1">
        <h4 className="text-sm font-semibold text-ink">SKILL.md{diff.body.changed ? "" : " (unchanged)"}</h4>
        <Lines old={diff.body.old} next={diff.body.new} label="SKILL.md diff" />
      </section>
      {diff.files.map((f) => (
        <section key={f.path} aria-label={`File ${f.path}`} className="space-y-1">
          <h4 className="text-sm font-semibold text-ink">
            <span className="font-mono">{f.path}</span>{" "}
            <span className="font-normal text-muted">
              ({f.change}
              {f.new_executable ? ", executable" : ""})
            </span>
          </h4>
          {f.binary ? (
            <p className="text-xs text-muted">Binary file: compared by content, not shown.</p>
          ) : f.change === "unchanged" ? null : (
            <Lines old={f.old ?? ""} next={f.new ?? ""} label={`Diff of ${f.path}`} />
          )}
        </section>
      ))}
    </div>
  );
}

// ---- update review ----------------------------------------------------------------------------

const KIND_LABEL: Record<string, string> = { changed: "Changed", removed: "Removed from the repository", invalid: "No longer valid" };

/**
 * The review of a proposed update (docs/14): the whole diff, the new commit, the kind, the budget impact;
 * Approve needs the tier's write authority (disabled, with the reason, for everybody else, who can still read
 * the diff) and quotes the exact content that was reviewed.
 */
export function UpdateReviewDialog({
  skill,
  tier,
  canDecide,
  open,
  onOpenChange,
}: {
  skill: Skill;
  tier: BudgetTier;
  canDecide: boolean;
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  const info = useSkillLibraryInfo(open ? skill.id : undefined);
  const approve = useApproveLibraryUpdate(skill.id);
  const reject = useRejectLibraryUpdate(skill.id);
  const update = info.data?.update;
  const failed = approve.error ?? reject.error;
  const superseded = failed instanceof ApiError && failed.code === "update_superseded";
  const blocked = !update || update.kind === "invalid" || update.over_budget || !canDecide;
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent
        title={`Update to ${skill.name}`}
        description="Review exactly what the agents will be told. Nothing changes until it is approved."
        className="w-[min(56rem,calc(100vw-2rem))]"
      >
        {info.isPending ? <Loading /> : null}
        {info.isError ? <ErrorState error={info.error} onRetry={() => info.refetch()} /> : null}
        {info.data && !update ? <EmptyState title="No update is waiting" /> : null}
        {info.data && update ? (
          <div className="space-y-4">
            <p className="flex flex-wrap items-center gap-2 text-sm text-ink">
              <Badge tone={update.kind === "changed" ? "pending" : update.kind === "invalid" ? "danger" : "neutral"}>{KIND_LABEL[update.kind] ?? update.kind}</Badge>
              <span>
                <span className="font-mono">{shortSha(info.data.commit_sha)}</span> → <span className="font-mono">{shortSha(update.commit_sha)}</span>
              </span>
            </p>
            {update.kind === "removed" ? (
              <Notice role="status">
                The repository no longer has this skill. Approving removes it from this {TIER_LABEL[tier].toLowerCase()}; rejecting keeps the pinned copy.
              </Notice>
            ) : null}
            {update.kind === "invalid" ? (
              <Notice tone="danger" role="status">
                The new version in the repository cannot be used{update.reason ? `: ${update.reason}` : ""}. It can only be rejected; the pinned copy keeps working.
              </Notice>
            ) : null}
            {update.over_budget ? (
              <Notice tone="pending" role="status">
                Approving would exceed this {TIER_LABEL[tier].toLowerCase()}&apos;s library budget ({LIBRARY_BUDGET[tier].skills} skills, {Math.round(LIBRARY_BUDGET[tier].bytes / 1024)}{" "}
                KiB). Remove a library skill or reject this update.
              </Notice>
            ) : null}
            {update.diff ? <DiffView diff={update.diff} /> : null}
            {!canDecide ? <Notice role="status">You can read this update. Only someone who can {AUTHORITY[tier]} can approve or reject it.</Notice> : null}
            {superseded ? (
              <Notice tone="pending" role="alert">
                {errorMessage(failed)}{" "}
                <Button
                  type="button"
                  size="sm"
                  variant="outline"
                  onClick={() => {
                    approve.reset();
                    reject.reset();
                    void info.refetch();
                  }}
                >
                  Reload the update
                </Button>
              </Notice>
            ) : failed ? (
              <Notice tone="danger" role="alert">
                {errorMessage(failed)}
              </Notice>
            ) : null}
            <div className="flex flex-wrap justify-end gap-2">
              <DialogClose asChild>
                <Button type="button" variant="outline">
                  Close
                </Button>
              </DialogClose>
              <Button type="button" variant="outline" disabled={!canDecide || reject.isPending} onClick={() => reject.mutate(update.id, { onSuccess: () => onOpenChange(false) })}>
                Reject
              </Button>
              <Button
                type="button"
                variant="approve"
                disabled={blocked || approve.isPending}
                onClick={() => approve.mutate({ update: update.id, contentSha256: update.content_sha256 }, { onSuccess: () => onOpenChange(false) })}
              >
                Approve
              </Button>
            </div>
          </div>
        ) : null}
      </DialogContent>
    </Dialog>
  );
}

/** Actions of a library skill: update review, detach (confirm). Remove is the ordinary delete. */
export function LibrarySkillActions({ skill, tier, canDecide }: { skill: Skill; tier: BudgetTier; canDecide: boolean }) {
  const [review, setReview] = useState(false);
  const [confirm, setConfirm] = useState(false);
  const detach = useDetachLibrarySkill(skill.id);
  const source = skill.source;
  if (!source || source.detached) return null;
  return (
    <div className="space-y-3">
      <div className="flex flex-wrap items-center gap-2">
        <LibraryBadge source={source} />
        {source.update_pending ? (
          <Button type="button" size="sm" variant="outline" onClick={() => setReview(true)}>
            Review update
          </Button>
        ) : null}
        {canDecide ? (
          confirm ? (
            <>
              <Button type="button" size="sm" variant="danger" disabled={detach.isPending} onClick={() => detach.mutate(undefined, { onSuccess: () => setConfirm(false) })}>
                Confirm detach
              </Button>
              <Button type="button" size="sm" variant="ghost" onClick={() => setConfirm(false)}>
                Cancel
              </Button>
            </>
          ) : (
            <Button type="button" size="sm" variant="outline" onClick={() => setConfirm(true)}>
              Detach
            </Button>
          )
        ) : null}
      </div>
      {confirm ? <Notice role="status">Detaching makes this an ordinary editable skill. The repository will no longer propose updates to it.</Notice> : null}
      {detach.error ? (
        <Notice tone="danger" role="alert">
          {errorMessage(detach.error)}
        </Notice>
      ) : null}
      <UpdateReviewDialog skill={skill} tier={tier} canDecide={canDecide} open={review} onOpenChange={setReview} />
    </div>
  );
}

// ---- Add from library -------------------------------------------------------------------------

function Preview({ content }: { content: LibrarySkillContent }) {
  return (
    <div className="space-y-3" aria-label="Preview of what will be stored">
      <p className="text-xs text-muted">
        Commit <span className="font-mono">{content.commit_sha}</span>
      </p>
      <section className="space-y-1">
        <h4 className="text-sm font-semibold text-ink">SKILL.md</h4>
        <pre aria-label="Description" className="whitespace-pre-wrap rounded-control border border-line bg-canvas p-2 text-xs text-ink [overflow-wrap:anywhere]">
          {content.description}
        </pre>
        <pre tabIndex={0} aria-label="SKILL.md body" className="max-w-full overflow-x-auto whitespace-pre-wrap rounded-control border border-line bg-canvas p-2 font-mono text-xs text-ink [overflow-wrap:anywhere]">
          {content.body}
        </pre>
      </section>
      {content.files.map((f) => (
        <section key={f.path} className="space-y-1">
          <h4 className="text-sm font-semibold text-ink">
            <span className="font-mono">{f.path}</span>{" "}
            <span className="font-normal text-muted">
              ({f.size.toLocaleString("en-US")} chars{f.executable ? ", executable" : ""})
            </span>
          </h4>
          {f.binary ? (
            <p className="text-xs text-muted">Binary file, not shown.</p>
          ) : (
            <pre tabIndex={0} aria-label={`File ${f.path}`} className="max-w-full overflow-x-auto whitespace-pre-wrap rounded-control border border-line bg-canvas p-2 font-mono text-xs text-ink [overflow-wrap:anywhere]">
              {f.content}
            </pre>
          )}
        </section>
      ))}
    </div>
  );
}

function AddedChips({ item }: { item: LibrarySkillItem }) {
  if (item.added.length === 0) return null;
  return (
    <>
      {item.added.map((a) => (
        <Badge key={a.id}>Already added · {TIER_LABEL[a.tier as SkillTier] ?? a.tier}</Badge>
      ))}
    </>
  );
}

/**
 * "Add from library…" (docs/14): picker → searchable list at the library's current commit → full preview
 * of what would be stored → Add (pinned to that commit). Shown only when the org has an active library.
 * `usage` is the tier's library-sourced bytes and count, to disable Add when it would not fit.
 */
export function AddFromLibrary({
  scope,
  orgId,
  tier,
  usage,
  onAdded,
}: {
  scope: SkillScope;
  orgId: string | undefined;
  tier: BudgetTier;
  usage: { bytes: number; count: number };
  onAdded: (id: string) => void;
}) {
  const libs = useSkillLibraries(orgId);
  const active = (libs.data?.items ?? []).filter((l) => l.status === "active");
  const [open, setOpen] = useState(false);
  const [libId, setLibId] = useState<string>("");
  const [q, setQ] = useState("");
  const [picked, setPicked] = useState<LibrarySkillItem | null>(null);
  const current = libId || active[0]?.id;
  const catalogue = useLibraryCatalogue(open ? current : undefined, q, "");
  const content = useLibrarySkill(open ? current : undefined, picked?.path, catalogue.data?.commit_sha);
  const create = useCreateSkill(scope);
  if (!orgId || active.length === 0) return null;
  const budget = LIBRARY_BUDGET[tier];
  const size = content.data?.size ?? 0;
  const over = !!content.data && (usage.bytes + size > budget.bytes || usage.count + 1 > budget.skills);

  async function add() {
    if (!current || !picked || !catalogue.data) return;
    try {
      const s = await create.mutateAsync({ from_library: { library_id: current, path: picked.path, commit_sha: catalogue.data.commit_sha } });
      setOpen(false);
      setPicked(null);
      onAdded(s.id);
    } catch {
      // shown below
    }
  }

  return (
    <>
      <Button
        type="button"
        variant="outline"
        onClick={() => {
          setOpen(true);
          setPicked(null);
          create.reset();
        }}
      >
        Add from library…
      </Button>
      <Dialog open={open} onOpenChange={setOpen}>
        <DialogContent title="Add from a library" description="Skills are pinned to the commit you see here." className="w-[min(48rem,calc(100vw-2rem))]">
          <div className="space-y-4">
            {active.length > 1 ? (
              <label className="block text-sm font-medium text-ink">
                Library
                <select
                  className={`${inputClass} mt-1`}
                  value={current}
                  onChange={(e) => {
                    setLibId(e.target.value);
                    setPicked(null);
                  }}
                >
                  {active.map((l) => (
                    <option key={l.id} value={l.id}>
                      {l.label}
                    </option>
                  ))}
                </select>
              </label>
            ) : null}
            {!picked ? (
              <>
                <label className="block text-sm font-medium text-ink">
                  Search skills
                  <input className={`${inputClass} mt-1`} value={q} onChange={(e) => setQ(e.target.value)} />
                </label>
                {catalogue.isPending ? <Loading label="Reading the repository…" /> : null}
                {catalogue.isError ? <ErrorState error={catalogue.error} onRetry={() => catalogue.refetch()} /> : null}
                {catalogue.data ? (
                  <>
                    <p className="text-xs text-muted">
                      At commit <span className="font-mono">{shortSha(catalogue.data.commit_sha)}</span>. Names and descriptions are repository text.
                    </p>
                    {catalogue.data.truncated ? <Notice role="status">This repository has more skills than can be listed. Connect a narrower folder.</Notice> : null}
                    {catalogue.data.items.length === 0 ? <EmptyState title="No skills found" /> : null}
                    <ul aria-label="Library skills" className="min-w-0 divide-y divide-line">
                      {catalogue.data.items.map((i) => (
                        <li key={i.path} className="py-2">
                          <button
                            type="button"
                            disabled={!i.valid}
                            className="block w-full min-w-0 text-left [overflow-wrap:anywhere] disabled:cursor-not-allowed disabled:opacity-60"
                            onClick={() => setPicked(i)}
                          >
                            <span className="font-mono text-sm text-ink">{i.name}</span>{" "}
                            <span className="text-xs text-muted">
                              {i.files} file{i.files === 1 ? "" : "s"}, about {i.size.toLocaleString("en-US")} characters
                            </span>{" "}
                            <AddedChips item={i} />
                            <span className="block text-xs text-ink-2">{i.valid ? i.description : <span className="text-danger-ink">Cannot be added: {i.reason}</span>}</span>
                          </button>
                        </li>
                      ))}
                    </ul>
                  </>
                ) : null}
              </>
            ) : (
              <>
                <p className="flex flex-wrap items-center gap-2">
                  <Button type="button" size="sm" variant="ghost" onClick={() => setPicked(null)}>
                    ← Back to the list
                  </Button>
                  <span className="font-mono text-sm font-semibold text-ink">{picked.name}</span>
                </p>
                {content.isPending ? <Loading /> : null}
                {content.isError ? <ErrorState error={content.error} onRetry={() => content.refetch()} /> : null}
                {content.data && !content.data.valid ? (
                  <Notice tone="danger" role="alert">
                    This skill cannot be added: {content.data.reason}
                  </Notice>
                ) : null}
                {content.data?.valid ? <Preview content={content.data} /> : null}
                <Notice role="status">Pinned to this commit. Changes in the repository are not applied until someone with permission approves them.</Notice>
                {over ? (
                  <Notice tone="pending" role="status">
                    This would exceed the {TIER_LABEL[tier].toLowerCase()} library budget: {(usage.bytes + size).toLocaleString("en-US")} of {budget.bytes.toLocaleString("en-US")} characters,{" "}
                    {usage.count + 1} of {budget.skills} skills.
                  </Notice>
                ) : null}
                {create.error ? (
                  <Notice tone="danger" role="alert">
                    {errorMessage(create.error)}
                  </Notice>
                ) : null}
                <div className="flex justify-end gap-2">
                  <DialogClose asChild>
                    <Button type="button" variant="outline">
                      Cancel
                    </Button>
                  </DialogClose>
                  <Button type="button" disabled={!content.data?.valid || over || create.isPending} onClick={() => void add()}>
                    Add
                  </Button>
                </div>
              </>
            )}
          </div>
        </DialogContent>
      </Dialog>
    </>
  );
}
