"use client";

import { useEffect, useRef, useState } from "react";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { ConfirmDialog } from "@/components/ui/confirm-dialog";
import { CopyChip } from "@/components/ui/copy-value";
import { LockIcon, MoreIcon, PencilIcon } from "@/components/ui/icons";
import { SubScreenHeader } from "@/components/ui/list-detail";
import { Markdown } from "@/components/ui/markdown";
import { Menu, MenuContent, MenuItem, MenuTrigger } from "@/components/ui/menu";
import { ErrorNotice } from "@/components/ui/notice";
import { useUndoToast } from "@/components/ui/undo-toast";
import { useWhenNow } from "@/components/ui/when";
import { useDeleteMemory, useSetMemoryLocked, type Memory } from "@/lib/api/memory";
import { formatExact } from "@/lib/format-date";
import { cut, memoryName } from "@/lib/memory-key";
import { cn } from "@/lib/utils";
import { personName } from "@/lib/workspace-name";

import { metaLine } from "./memory-words";

/** What a search found in the memory on screen. */
export interface MemoryMatch {
  /** The words to mark. */
  terms: string[];
  /** Which result this is, from 1, and how many there are. */
  index: number;
  total: number;
}

/** "Locked by Ada on <date>" ("Locked on <date>" without the locker); null when the memory is not locked. */
export function lockedWords(memory: Memory): string | null {
  if (!memory.locked || !memory.locked_at) return null;
  return memory.locked_by
    ? `Locked by ${personName(memory.locked_by.display_name)} on ${formatExact(memory.locked_at)}`
    : `Locked on ${formatExact(memory.locked_at)}`;
}

/** The first line of the text, as written, to say where the shown part starts. */
function firstLine(content: string): string {
  return cut((content.split("\n").find((l) => l.trim()) ?? "").trim(), 40).replace(/…$/, "");
}

/**
 * (P5-MEMORY-2, M-2) One memory, rendered: its title, description and key, the text as Markdown, and the line saying
 * who added it, how often it changed and when the agent last used it. Edit, Lock and "…" (History, Delete) are its
 * actions; on a narrow screen Edit and "…" sit beside the way back and Lock and History at the bottom. An emergency
 * reader (`readOnly`) gets the same view with no actions.
 */
export function MemoryView({
  agentId,
  memory,
  agentName,
  me,
  readOnly,
  narrow,
  match,
  historyOpen,
  onBack,
  onEdit,
  onHistory,
  onDeleted,
}: {
  agentId: string;
  memory: Memory;
  agentName: string;
  me: string | null | undefined;
  readOnly: boolean;
  narrow: boolean;
  match: MemoryMatch | null;
  historyOpen: boolean;
  onBack: () => void;
  onEdit: () => void;
  onHistory: () => void;
  onDeleted: () => void;
}) {
  const now = useWhenNow();
  const lock = useSetMemoryLocked(agentId, memory.key);
  const del = useDeleteMemory(agentId);
  const toast = useUndoToast();
  const [confirmDelete, setConfirmDelete] = useState(false);
  const body = useRef<HTMLDivElement>(null);
  const terms = match?.terms.join("\u0000") ?? "";
  const lower = memory.content.toLowerCase();
  const inText = Boolean(match?.terms.some((t) => lower.includes(t.toLowerCase())));

  // The first match is brought into view.
  useEffect(() => {
    if (!terms) return;
    const el = body.current?.querySelector(".md-match-line") ?? body.current?.querySelector("mark");
    el?.scrollIntoView?.({ block: "nearest" });
  }, [terms, memory.key, memory.revision]);

  async function toggleLock() {
    const on = !memory.locked;
    try {
      await lock.mutateAsync(on);
      toast(on ? "Locked" : "Unlocked", undefined, { success: true });
    } catch {
      // shown by lock.error
    }
  }

  const editButton = (
    <Button type="button" variant="outline" size="sm" className="max-md:min-h-11" onClick={onEdit}>
      <PencilIcon className="size-4" />
      Edit
    </Button>
  );
  const lockButton = (className?: string) => (
    <Button type="button" variant="outline" size="sm" className={cn("max-md:min-h-11", className)} disabled={lock.isPending} onClick={() => void toggleLock()}>
      <LockIcon className="size-4" />
      {memory.locked ? "Unlock" : "Lock"}
    </Button>
  );
  const menu = (
    <Menu>
      <MenuTrigger asChild>
        <Button type="button" variant={narrow ? "ghost" : "outline"} size="sm" className="px-2.5 max-md:min-h-11 max-md:min-w-11" aria-label="More actions">
          <MoreIcon className="size-5" strokeWidth={3} />
        </Button>
      </MenuTrigger>
      <MenuContent align="end">
        {narrow ? null : <MenuItem onSelect={onHistory}>{historyOpen ? "Hide history" : "History"}</MenuItem>}
        <MenuItem className="text-danger-ink" onSelect={() => setConfirmDelete(true)}>
          Delete
        </MenuItem>
      </MenuContent>
    </Menu>
  );
  const name = memoryName(memory);
  const locked = lockedWords(memory);

  const article = (
    <>
      <div className={cn("space-y-2", narrow ? "" : "border-b border-line p-5")}>
        <div className="flex items-start justify-between gap-3">
          <div className="min-w-0 flex-1 space-y-1">
            <h3 className="break-words font-display text-lg font-semibold leading-6 text-ink">{name}</h3>
            {memory.description?.trim() ? <p className="break-words text-sm text-ink-2">{memory.description.trim()}</p> : null}
          </div>
          {!readOnly && !narrow ? (
            <div className="flex shrink-0 items-center gap-1.5">
              {editButton}
              {lockButton()}
              {menu}
            </div>
          ) : null}
        </div>
        <div className="flex flex-wrap items-center gap-2">
          <CopyChip label="Key" value={memory.key} />
          {memory.locked && (readOnly || narrow) ? (
            <Badge>
              <LockIcon className="size-3" aria-hidden="true" /> Locked
            </Badge>
          ) : null}
        </div>
      </div>
      <div ref={body} className={cn("md-reading min-w-0 space-y-3", narrow ? "py-4" : "p-5")}>
        {match ? (
          <p className="flex flex-wrap items-center gap-2 text-xs text-muted" data-testid="match-line">
            <Badge tone="accent">
              Match {match.index} of {match.total}
            </Badge>
            {inText ? <span className="min-w-0 break-words">in the text, from the top: “{firstLine(memory.content)} …”</span> : null}
          </p>
        ) : null}
        <Markdown source={memory.content} marks={match?.terms} reading />
      </div>
      <div className={cn("border-t border-line text-xs text-muted", narrow ? "pt-3" : "px-5 py-3")}>
        <p className="break-words" data-testid="memory-meta">
          {[metaLine(memory, agentName, me, now), locked].filter(Boolean).join(" · ")}
        </p>
      </div>
    </>
  );

  return (
    <div className="min-w-0 space-y-3">
      {narrow ? (
        <SubScreenHeader backLabel="Memory" onBack={onBack}>
          {readOnly ? null : (
            <>
              {editButton}
              {menu}
            </>
          )}
        </SubScreenHeader>
      ) : null}
      {narrow ? <article aria-label={name}>{article}</article> : <Card role="article" aria-label={name} className="p-0 sm:p-0">{article}</Card>}
      {narrow && !readOnly ? (
        <div className="grid grid-cols-2 gap-2">
          {lockButton("w-full")}
          <Button type="button" variant="outline" size="sm" className="min-h-11 w-full" aria-expanded={historyOpen} onClick={onHistory}>
            History
          </Button>
        </div>
      ) : null}
      <ErrorNotice error={lock.error ?? del.error} />
      <ConfirmDialog
        open={confirmDelete}
        onOpenChange={setConfirmDelete}
        title={`Delete the memory “${name}”?`}
        description="The agent forgets it, and its history goes with it. This can't be undone."
        confirmLabel="Delete memory"
        danger
        busy={del.isPending}
        onConfirm={() => {
          setConfirmDelete(false);
          void del.mutateAsync(memory.key).then(onDeleted, () => undefined);
        }}
      />
    </div>
  );
}
