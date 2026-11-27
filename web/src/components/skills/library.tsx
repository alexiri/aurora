"use client";

import { skillDisplayName } from "@/lib/skill-name";

import { EmptyState, ErrorState, Loading } from "@/components/states";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Dialog, DialogClose, DialogContent } from "@/components/ui/dialog";
import { ErrorNotice, Notice } from "@/components/ui/notice";
import { ApiError } from "@/lib/api/client";
import { errorMessage } from "@/lib/api/errors";
import {
  shortSha,
  useApproveLibraryUpdate,
  usePendingLibraryUpdates,
  useRejectLibraryUpdate,
  useSkillLibraryInfo,
  type BudgetTier,
  type LibraryDiff,
} from "@/lib/api/skill-libraries";
import { TIER_LABEL, type Skill, type SkillScope } from "@/lib/api/skills";

import { DiffView as FileDiffView } from "@/components/ui/diff-view";

// Everything from a repository is untrusted text: here it is only ever rendered as React text nodes.

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

const AUTHORITY: Record<BudgetTier, string> = {
  agent: "use this agent",
  workspace: "own this workspace",
  org: "be an organization owner or admin",
};

// ---- the whole diff ---------------------------------------------------------------------------

/** The whole stored-versus-proposed content: the folded GitLab-style diff (FIX-250), nothing summarized. */
export function DiffView({ diff }: { diff: LibraryDiff }) {
  return (
    <div className="space-y-4">
      <section aria-label="Description change" className="space-y-1">
        <h4 className="text-sm font-semibold text-ink">Description{diff.description.changed ? "" : " (unchanged)"}</h4>
        <FileDiffView old={diff.description.old} next={diff.description.new} label="Description diff" fileName="description" />
      </section>
      <section aria-label="SKILL.md change" className="space-y-1">
        <h4 className="text-sm font-semibold text-ink">SKILL.md{diff.body.changed ? "" : " (unchanged)"}</h4>
        <FileDiffView old={diff.body.old} next={diff.body.new} label="SKILL.md diff" fileName="SKILL.md" />
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
            <FileDiffView old={f.old ?? ""} next={f.new ?? ""} label={`Diff of ${f.path}`} fileName={f.path} status={f.change === "added" ? "added" : f.change === "removed" ? "removed" : null} />
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
  skill: Pick<Skill, "id" | "name" | "title">;
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
        title={`Update to ${skillDisplayName(skill.name, skill.title)}`}
        description="Review exactly what the agents will be told. Nothing changes until it is approved."
        size="2xl"
      >
        {info.isPending ? <Loading /> : null}
        {info.isError ? <ErrorState error={info.error} onRetry={() => info.refetch()} /> : null}
        {info.data && !update ? <EmptyState title="No update is waiting" /> : null}
        {info.data && update ? (
          <div className="space-y-4">
            <p className="flex flex-wrap items-center gap-2 text-sm text-ink">
              <Badge tone={update.kind === "invalid" ? "danger" : "neutral"}>{KIND_LABEL[update.kind] ?? update.kind}</Badge>
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
                Approving would exceed this {TIER_LABEL[tier].toLowerCase()}&apos;s library budget. Remove a library skill or reject this update.
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
              <ErrorNotice error={failed} />
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

