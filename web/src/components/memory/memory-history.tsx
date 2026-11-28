"use client";

import { useMemo, useState } from "react";

import { Avatar } from "@/components/agent-avatar";
import { ErrorState } from "@/components/states";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { ConfirmDialog } from "@/components/ui/confirm-dialog";
import { DiffView } from "@/components/ui/diff-view";
import { ChevronLeftIcon, MoreIcon } from "@/components/ui/icons";
import { SubScreenHeader } from "@/components/ui/list-detail";
import { Menu, MenuContent, MenuItem, MenuTrigger } from "@/components/ui/menu";
import { ErrorNotice } from "@/components/ui/notice";
import { SkeletonRows } from "@/components/ui/skeleton";
import { showUndoToast } from "@/components/ui/undo-toast";
import { When, useWhenNow } from "@/components/ui/when";
import { errorMessage } from "@/lib/api/errors";
import { canRedact, useMemoryRevisions, useRedactRevision, useRestoreMemory, type Memory, type MemoryRevision } from "@/lib/api/memory";
import { whenInSentence } from "@/lib/format-date";
import { buildDiff, expandAll, type DiffLineKind } from "@/lib/line-diff";
import { memoryName } from "@/lib/memory-key";
import { cn } from "@/lib/utils";

import { versionActor, versionContext, versionVerb } from "./memory-words";

const CURRENT_REASON = "The current version can't be removed: change the memory first.";
const REMOVE_LABEL = "Remove this version's text…";

/** How many lines `next` adds to and removes from `old`; null when the texts are too large to compare. */
export function changeCounts(old: string, next: string): { added: number; removed: number } | null {
  const diff = buildDiff(old, next);
  if (diff.tooLarge) return null;
  const kinds = expandAll(diff.rows).flatMap((r): DiffLineKind[] => (r.type === "line" ? [r.line.kind] : []));
  return { added: kinds.filter((k) => k === "add").length, removed: kinds.filter((k) => k === "del").length };
}

/**
 * (P5-MEMORY-3, M-5, mem-4) A memory's History, in place of the reading view: the versions newest first, named by who
 * wrote them and where; what the chosen one changed, as the shared `DiffView` against the version before; **Restore
 * this version** (undoable, so it does not ask) and **Remove this version's text…** (it asks: it can't be undone). On a
 * narrow screen it is its own screen.
 */
export function MemoryHistory({
  agentId,
  memory,
  agentName,
  narrow,
  onBack,
  onRestored,
}: {
  agentId: string;
  memory: Memory;
  agentName: string;
  narrow: boolean;
  onBack: () => void;
  onRestored: () => void;
}) {
  const revs = useMemoryRevisions(agentId, memory.key, true);
  const restore = useRestoreMemory(agentId, memory.key);
  const redact = useRedactRevision(agentId, memory.key);
  const now = useWhenNow();
  const [chosen, setChosen] = useState<number | null>(null);
  const [removing, setRemoving] = useState<number | null>(null);

  const versions = useMemo(() => [...(revs.data?.items ?? [])].sort((a, b) => b.revision - a.revision), [revs.data]);
  const at = Math.max(0, versions.findIndex((v) => v.revision === chosen));
  const selected: MemoryRevision | undefined = versions[at];
  const before: MemoryRevision | undefined = versions[at + 1];
  const name = memoryName(memory);

  async function restoreSelected(revision: number) {
    const wasCurrent = memory.revision;
    try {
      await restore.mutateAsync(revision);
    } catch {
      return; // shown by restore.error
    }
    onRestored();
    showUndoToast(
      `Restored version ${revision}`,
      () => {
        restore.mutateAsync(wasCurrent).catch((e: unknown) => showUndoToast(`Couldn't undo the restore: ${errorMessage(e)}`));
      },
      { success: true },
    );
  }

  const body = (
    <>
      <div className={cn("space-y-1.5", narrow ? "py-3" : "px-5 py-4")}>
        {narrow ? null : (
          <button type="button" onClick={onBack} className="inline-flex min-h-9 items-center gap-1 text-sm font-semibold text-accent-ink">
            <ChevronLeftIcon className="size-4" aria-hidden="true" />
            Back to the memory
          </button>
        )}
        <h3 className="font-display text-xl font-semibold text-ink">History</h3>
        <p className="break-words text-sm text-ink-2">
          {revs.data ? `${name} · ${versions.length === 1 ? "1 version" : `${versions.length} versions`}` : name}
        </p>
      </div>
      {revs.isPending ? <SkeletonRows rows={3} label="Loading the history…" className="rounded-none border-x-0" /> : null}
      {revs.isError ? (
        <div className={narrow ? "" : "px-5 pb-4"}>
          <ErrorState error={revs.error} onRetry={() => revs.refetch()} />
        </div>
      ) : null}
      {versions.length ? (
        <ul aria-label="Versions" className="divide-y divide-line border-y border-line">
          {versions.map((v) => {
            const who = versionActor(v, agentName);
            const context = versionContext(v);
            return (
              <li key={v.revision}>
                <button
                  type="button"
                  aria-current={v.revision === selected?.revision}
                  onClick={() => {
                    restore.reset();
                    setChosen(v.revision);
                  }}
                  className="flex w-full min-w-0 items-start gap-2.5 row-rail border-l-transparent px-3 py-2.5 text-left hover:bg-subtle aria-[current=true]:border-l-accent aria-[current=true]:bg-subtle"
                >
                  <Avatar name={who} size={28} />
                  <span className="min-w-0 flex-1">
                    <span className="block break-words text-sm text-ink">
                      <strong className="font-semibold">{who}</strong> {versionVerb(v)} ·{" "}
                      <When value={v.created_at} className="text-ink-2" />
                      {v.revision === memory.revision ? (
                        <Badge tone="accent" className="ml-2">
                          Current
                        </Badge>
                      ) : null}
                    </span>
                    {context ? <span className="block break-words text-xs text-muted">{context}</span> : null}
                  </span>
                </button>
              </li>
            );
          })}
        </ul>
      ) : null}
      {selected ? (
        <Changed
          key={selected.revision}
          selected={selected}
          before={before}
          beforeWords={before ? `${versionActor(before, agentName)}, ${whenInSentence(before.created_at, { now })}` : null}
          narrow={narrow}
          current={memory.revision}
          restoring={restore.isPending}
          error={restore.error}
          onRestore={() => void restoreSelected(selected.revision)}
          onRemove={() => {
            redact.reset();
            setRemoving(selected.revision);
          }}
        />
      ) : null}
    </>
  );

  return (
    <div className="min-w-0 space-y-3">
      {narrow ? <SubScreenHeader backLabel="Back to the memory" onBack={onBack} /> : null}
      {narrow ? (
        <article aria-label="History">{body}</article>
      ) : (
        <Card role="article" aria-label="History" className="p-0 sm:p-0">
          {body}
        </Card>
      )}
      <ConfirmDialog
        open={removing !== null}
        onOpenChange={(o) => (o ? undefined : setRemoving(null))}
        title={`Remove the text of version ${removing ?? ""}?`}
        description="The text of this version is deleted for good; the other versions and the list of changes stay. This can't be undone."
        confirmLabel="Remove text"
        danger
        busy={redact.isPending}
        onConfirm={() => {
          if (removing !== null) void redact.mutateAsync(removing).then(() => setRemoving(null), () => undefined);
        }}
      >
        <ErrorNotice error={redact.error} />
      </ConfirmDialog>
    </div>
  );
}

/** What the chosen version changed, with what can be done with it. */
function Changed({
  selected,
  before,
  beforeWords,
  narrow,
  current,
  restoring,
  error,
  onRestore,
  onRemove,
}: {
  selected: MemoryRevision;
  before: MemoryRevision | undefined;
  /** "Daneel, yesterday": who wrote the version before, and when. */
  beforeWords: string | null;
  narrow: boolean;
  current: number;
  restoring: boolean;
  error: unknown;
  onRestore: () => void;
  onRemove: () => void;
}) {
  const first = selected.revision === 1;
  const removed = selected.redacted || Boolean(before?.redacted);
  // The oldest kept version is not the first one: what came before it is gone, so there is nothing to compare with.
  const pruned = !before && !first;
  const texts = removed || pruned ? null : { old: before?.content ?? "", next: selected.content ?? "" };
  const counts = texts ? changeCounts(texts.old, texts.next) : null;
  const isCurrent = selected.revision === current;
  const pad = narrow ? "" : "px-5";
  return (
    <>
      <section aria-label="What changed" className={cn("space-y-3 py-4", pad)}>
        <div className="flex flex-wrap items-center justify-between gap-2">
          <div className="min-w-0 flex-1 basis-40">
            <h4 className="text-sm font-semibold text-ink">What changed</h4>
            {beforeWords ? (
              <p className="break-words text-xs text-muted">Compared with the version before ({beforeWords})</p>
            ) : first ? (
              <p className="text-xs text-muted">The first version</p>
            ) : null}
          </div>
          <div className="flex shrink-0 items-center gap-1.5">
            {!isCurrent && !selected.redacted ? (
              <Button type="button" variant="outline" size="sm" className="max-md:min-h-11" disabled={restoring} onClick={onRestore}>
                Restore this version
              </Button>
            ) : null}
            {selected.redacted ? null : (
              <Menu>
                <MenuTrigger asChild>
                  <Button type="button" variant="outline" size="sm" className="px-2.5 max-md:min-h-11 max-md:min-w-11" aria-label="More actions">
                    <MoreIcon className="size-5" strokeWidth={3} />
                  </Button>
                </MenuTrigger>
                <MenuContent align="end" className="max-w-72">
                  {canRedact(selected, current) ? (
                    <MenuItem className="text-danger-ink" onSelect={onRemove}>
                      {REMOVE_LABEL}
                    </MenuItem>
                  ) : (
                    // Said in the item itself, so it is read with it; not dimmed as a whole, so the reason stays legible.
                    <MenuItem disabled className="flex-col items-start data-[disabled]:cursor-default data-[disabled]:opacity-100">
                      <span className="text-muted">{REMOVE_LABEL}</span>
                      <span className="text-xs text-ink-2">{CURRENT_REASON}</span>
                    </MenuItem>
                  )}
                </MenuContent>
              </Menu>
            )}
          </div>
        </div>
        <ErrorNotice error={error} />
        {removed ? <p className="text-sm text-muted">This version&apos;s text was removed</p> : null}
        {pruned && !removed ? <p className="text-sm text-muted">The version before is no longer kept, so there is nothing to compare with.</p> : null}
        {texts ? (
          <DiffView old={texts.old} next={texts.next} label={`Diff of version ${selected.revision}${before ? ` against version ${before.revision}` : ""}`} />
        ) : null}
      </section>
      {counts ? (
        <p className={cn("border-t border-line py-3 text-xs text-muted", pad)} data-testid="diff-summary">
          <span className="text-accent-ink">+{counts.added} added</span> · <span className="text-danger-ink">−{counts.removed} removed</span> in this
          version
        </p>
      ) : null}
    </>
  );
}
