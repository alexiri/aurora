"use client";

import Link from "next/link";
import { useRouter } from "next/navigation";
import { useRef, useState } from "react";
import { useQuery } from "@tanstack/react-query";

import { Button } from "@/components/ui/button";
import { BookmarkIcon, LockIcon } from "@/components/ui/icons";
import { ErrorNotice } from "@/components/ui/notice";
import { useUndoToast } from "@/components/ui/undo-toast";
import { api, ApiError } from "@/lib/api/client";
import type { TranscriptEvent } from "@/lib/api/conversations";
import { memKeys, useCreateMemory, useDeleteMemory, useRestoreMemory, type Memory } from "@/lib/api/memory";
import { useWorkspace } from "@/lib/api/queries";
import { unwrap } from "@/lib/api/unwrap";
import { setMemoryDraft } from "@/lib/chat/memory-draft";
import type { TranscriptState } from "@/lib/chat/transcript";
import { cn } from "@/lib/utils";

import type { AgentIdent } from "./types";

const str = (v: unknown): string => (typeof v === "string" ? v : "");
const num = (v: unknown): number | null => (typeof v === "number" ? v : null);

/** The memory as it is now; null when there is none under this key. */
async function readMemory(agentId: string, key: string): Promise<Memory | null> {
  try {
    return await unwrap(api.GET("/v1/agents/{agent}/memories/{key}", { params: { path: { agent: agentId, key } } }));
  } catch (error) {
    if (error instanceof ApiError && error.status === 404) return null;
    throw error;
  }
}

/** How long after the card's event a memory may say it was created and still be the one the event is about. */
const CREATED_SLACK_MS = 1000;

interface Props {
  ev: TranscriptEvent;
  state: TranscriptState;
  wsId: string;
  /** `/o/<org>/w/<workspace>` */
  base: string;
  agent?: AgentIdent;
}

/**
 * Who may do what with the agent's memories, from the workspace alone: own access reads and edits, emergency read
 * access reads. The API decides again on every request.
 */
function useMemoryAccess(wsId: string) {
  const ws = useWorkspace(wsId).data;
  const emergency = ws?.emergency_reader === true;
  const own = Boolean(ws?.my_role);
  return { mayRead: own || emergency, mayEdit: own && !emergency };
}

/** (mem-5, C-1172) What the agent remembered, or tried to, under the tool calls of its turn. */
export function MemoryCard(props: Props) {
  return props.ev.type === "memory.saved" ? <SavedCard {...props} /> : <RefusedCard {...props} />;
}

const memoryHref = (base: string, handle: string, key: string | null) =>
  `${base}/agents/${handle}?tab=memory${key ? `&memory=${encodeURIComponent(key)}` : ""}`;

/** What this card's save looks like now: undone or not, and which memory that should be. */
interface Mark {
  undone: boolean;
  /** The revision that carries this card's change (or, once undone, its reverse). */
  revision: number;
  /** Null until this card wrote the memory itself: then the memory must be older than the event. */
  createdAt: string | null;
  /** The memory as it was when a created one was deleted: what Redo writes again. */
  deleted?: Memory;
}

function SavedCard({ ev, state, wsId, base, agent }: Props) {
  const p = ev.payload;
  const key = str(p.key);
  const created = p.created === true;
  const previous = num(p.previous_revision);
  const savedRevision = num(p.revision) ?? 0;
  const name = agent?.name ?? "Agent";
  const agentId = agent?.id ?? "";
  const { mayRead, mayEdit } = useMemoryAccess(wsId);
  const toast = useUndoToast();
  const [mark, setMark] = useState<Mark>({ undone: false, revision: savedRevision, createdAt: null });
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<unknown>(null);
  const running = useRef(false);

  // A later save of the same memory makes an earlier card read it again.
  const latest = state.events.reduce((max, e) => (e.type === "memory.saved" && e.payload.key === key ? Math.max(max, e.seq ?? 0) : max), 0);
  // Only someone who may undo needs the memory as it is now. (An emergency reader's reads are recorded: none are made here.)
  const now = useQuery({
    queryKey: [...memKeys.list(agentId), "chat", key, latest],
    queryFn: () => readMemory(agentId, key),
    enabled: Boolean(agentId) && mayEdit,
    retry: false,
  });
  const del = useDeleteMemory(agentId);
  const restore = useRestoreMemory(agentId, key);
  const create = useCreateMemory(agentId);

  /** Whether `m` is still what this card last knew: nothing newer would be overwritten. */
  const untouched = (m: Memory | null): boolean => {
    if (mark.undone && created) return m === null;
    if (!m || m.revision !== mark.revision) return false;
    // A memory deleted and made again under the same key starts its revisions again: its creation time tells.
    return mark.createdAt ? m.created_at === mark.createdAt : Date.parse(m.created_at) <= Date.parse(ev.ts) + CREATED_SLACK_MS;
  };

  async function act() {
    if (running.current) return;
    running.current = true;
    setBusy(true);
    setError(null);
    try {
      // Read again at the click: what is on screen may be old, and a newer edit is never overwritten.
      const fresh = await now.refetch();
      if (fresh.isError) throw fresh.error;
      const current = fresh.data ?? null;
      if (!untouched(current)) return;
      if (!mark.undone) {
        if (created) {
          await del.mutateAsync(key);
          setMark({ undone: true, revision: 0, createdAt: null, deleted: current! });
        } else {
          const m = await restore.mutateAsync(previous!);
          setMark({ undone: true, revision: m.revision, createdAt: m.created_at });
        }
        toast("Memory undone", undefined, { success: true });
        return;
      }
      if (created) {
        // Deleting a memory takes its revisions with it, so there is nothing to restore: it is written again.
        const was = mark.deleted!;
        const m = await create.mutateAsync({
          key,
          title: was.title ?? null,
          description: was.description ?? null,
          content: was.content,
          ...(was.locked ? { locked: true } : {}),
        });
        setMark({ undone: false, revision: m.revision, createdAt: m.created_at });
      } else {
        // The saved revision is the one before the Undo's own.
        const m = await restore.mutateAsync(mark.revision - 1);
        setMark({ undone: false, revision: m.revision, createdAt: m.created_at });
      }
      toast("Memory saved again", undefined, { success: true });
    } catch (e) {
      // Someone made a memory under this key meanwhile: the card shows it as changed.
      if (e instanceof ApiError && e.code === "memory_exists") await now.refetch();
      else setError(e);
    } finally {
      running.current = false;
      setBusy(false);
    }
  }

  const known = mayEdit && now.data !== undefined;
  // While an action runs the memory moves under the card: nothing is judged until it is done.
  const changed = known && !busy && !untouched(now.data ?? null);
  const gone = known && now.data === null;
  const undoable = known && (created || previous !== null);
  const reason = gone ? "This memory was deleted after this was saved." : "This memory changed after this was saved.";
  const showView = mayRead && Boolean(agent?.handle) && !gone;
  const title = str(p.title) || key;
  const actionClass = "max-md:min-h-11";

  return (
    <div data-testid="memory-card" role="group" aria-label={`${name} remembered ${title}`} className="max-w-142.5 space-y-2">
      <div className="flex flex-wrap items-center gap-3 rounded-card border border-line-strong bg-surface px-3.5 py-3 dark:bg-canvas">
        <span aria-hidden="true" className="grid size-9 shrink-0 place-items-center rounded-control bg-accent-soft text-accent-ink">
          <BookmarkIcon className="size-5" />
        </span>
        <div className="min-w-0 flex-1 basis-44">
          <p className="text-xs font-semibold uppercase tracking-wide text-accent-ink">{name} remembered</p>
          <p className="break-words text-sm font-semibold text-ink">{title}</p>
          {str(p.preview) ? <p className="break-words text-sm text-ink-2 md:truncate">{str(p.preview)}</p> : null}
          {changed ? <p className="text-xs text-muted">{reason}</p> : null}
        </div>
        {showView || undoable ? (
          <div className="ml-auto flex shrink-0 items-center gap-2">
            {showView ? (
              <Button asChild variant="outline" size="sm" className={actionClass}>
                <Link href={memoryHref(base, agent!.handle!, key)}>View</Link>
              </Button>
            ) : null}
            {!undoable ? null : changed ? (
              <Button type="button" variant="ghost" size="sm" className={actionClass} disabled title={reason}>
                Changed since
              </Button>
            ) : mark.undone ? (
              <span className="flex items-center gap-1 text-sm text-muted">
                Undone ·
                <Button type="button" variant="ghost" size="sm" className={actionClass} disabled={busy} onClick={() => void act()}>
                  Redo
                </Button>
              </span>
            ) : (
              <Button type="button" variant="ghost" size="sm" className={actionClass} disabled={busy} onClick={() => void act()}>
                Undo
              </Button>
            )}
          </div>
        ) : null}
      </div>
      <ErrorNotice error={error} />
    </div>
  );
}

function RefusedCard({ ev, wsId, base, agent }: Props) {
  const p = ev.payload;
  const key = str(p.key);
  const preview = str(p.preview);
  const locked = str(p.code) === "memory_locked";
  const name = agent?.name ?? "Agent";
  const { mayEdit } = useMemoryAccess(wsId);
  const router = useRouter();
  // For this viewer, until the page is loaded again.
  const [dismissed, setDismissed] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<unknown>(null);
  const heading = `${name} tried to remember this but couldn't`;
  const Icon = locked ? LockIcon : BookmarkIcon;

  async function saveYourself() {
    if (!agent?.handle) return;
    setBusy(true);
    setError(null);
    try {
      const current = key ? await readMemory(agent.id, key) : null;
      if (locked && current) {
        setMemoryDraft(agent.id, { kind: "edit", key, content: [current.content.trimEnd(), preview].filter(Boolean).join("\n\n") });
        router.push(memoryHref(base, agent.handle, key));
      } else {
        setMemoryDraft(agent.id, { kind: "new", key: key && !current ? key : null, title: str(p.title), content: preview });
        router.push(memoryHref(base, agent.handle, null));
      }
    } catch (e) {
      setError(e);
    } finally {
      setBusy(false);
    }
  }

  const ghost = "text-pending-ink hover:bg-pending-soft hover:text-pending-ink max-md:min-h-11";
  return (
    <div data-testid="memory-card" role="group" aria-label={heading} className="max-w-142.5 space-y-2">
      <div className={cn("flex gap-3 rounded-card border border-pending-line bg-pending-panel px-3.5 text-sm text-pending-ink", dismissed ? "items-center py-1" : "items-start py-3")}>
        <Icon aria-hidden="true" className={cn("shrink-0", !dismissed && "mt-0.5")} />
        {dismissed ? (
          <div className="flex min-w-0 flex-1 flex-wrap items-center justify-between gap-x-3">
            <p className="min-w-0 break-words">{heading}</p>
            <Button type="button" variant="ghost" size="sm" className={ghost} onClick={() => setDismissed(false)}>
              Show
            </Button>
          </div>
        ) : (
          <div className="min-w-0 flex-1">
            <p className="break-words font-semibold">{heading}</p>
            <p className="mt-0.5 break-words">
              {str(p.message)}
              {preview ? " What it wanted to add:" : ""}
            </p>
            {preview ? <p className="mt-1.5 whitespace-pre-wrap break-words rounded-control bg-pending-soft px-2.5 py-2">{preview}</p> : null}
            <div className="mt-2.5 flex flex-wrap gap-2">
              {mayEdit && agent?.handle ? (
                <Button type="button" variant="pending" size="sm" className="max-md:min-h-11" disabled={busy} onClick={() => void saveYourself()}>
                  Save it yourself
                </Button>
              ) : null}
              <Button type="button" variant="ghost" size="sm" className={ghost} onClick={() => setDismissed(true)}>
                Dismiss
              </Button>
            </div>
          </div>
        )}
      </div>
      <ErrorNotice error={error} />
    </div>
  );
}
