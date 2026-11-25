"use client";

import { useState, type FormEvent } from "react";

import { EmptyState, ErrorState, Loading } from "@/components/states";
import { Altitude } from "@/components/ui/altitude";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { ConfirmAction } from "@/components/ui/confirm-dialog";
import { CheckboxField, FormActions } from "@/components/ui/form";
import { Field, inputClass } from "@/components/ui/field";
import { LockIcon } from "@/components/ui/icons";
import { ErrorNotice, Notice } from "@/components/ui/notice";
import { ItemList } from "@/components/ui/section";
import { SwitchField } from "@/components/ui/switch";
import { TechnicalDetails } from "@/components/ui/technical-details";
import { When } from "@/components/ui/when";
import { useUndoToast } from "@/components/ui/undo-toast";
import { cn } from "@/lib/utils";
import { ApiError } from "@/lib/api/client";
import { formatExact } from "@/lib/format-date";
import { personName } from "@/lib/workspace-name";
import { memoryName, memoryPreview, newMemoryKey } from "@/lib/memory-key";
import {
  MEMORY_CONTENT_MAX,
  canRedact,
  useCreateMemory,
  useDeleteMemory,
  useMemories,
  useMemoryRevisions,
  useMemorySearch,
  usePutMemory,
  useRedactRevision,
  useRestoreMemory,
  useSetMemoryLocked,
  type Memory,
} from "@/lib/api/memory";

import { SizeMeter } from "./size-meter";

const memItem = "block min-h-11 w-full px-3 py-2 text-left text-sm hover:bg-subtle sm:px-4";

function capitalize(s: string) {
  return s.charAt(0).toUpperCase() + s.slice(1);
}

export function MemoryTab({ agentId, memoryLanguage, agentName, readOnly = false }: { agentId: string; memoryLanguage?: string; agentName?: string; /** Emergency read access: memories can be read and searched, not written. */ readOnly?: boolean }) {
  const list = useMemories(agentId);
  const [q, setQ] = useState("");
  const search = useMemorySearch(agentId, q);
  const [selected, setSelected] = useState<string | null>(null);
  const [creating, setCreating] = useState(false);
  const items = list.data?.items ?? [];
  const current = items.find((m) => m.key === selected);

  // (P5-POLISH-6, C6) No memory yet: the whole tab is one empty state with its one action, and no "Select a memory" pane.
  if (list.data && items.length === 0 && !creating && !readOnly) {
    return (
      <EmptyState
        title={`${agentName ?? "This agent"} doesn't remember anything yet`}
        when="It saves notes here as you work together. You can also write one yourself."
        action={
          <Button type="button" onClick={() => { setCreating(true); setSelected(null); }}>
            New memory
          </Button>
        }
      />
    );
  }
  // Emergency read access: no memory yet means just that; nothing to write here.
  if (list.data && items.length === 0 && readOnly) {
    return <EmptyState title={`${agentName ?? "This agent"} doesn't remember anything yet`} when="It saves notes here as you work together." />;
  }

  return (
    <div className="space-y-4">
      {memoryLanguage ? (
        <Notice role="note">Memories in this workspace are written in {capitalize(memoryLanguage)}.</Notice>
      ) : null}
      <div className="flex flex-wrap items-end gap-2">
        <div className="min-w-0 flex-1 basis-56">
          <Field id="mem-search" label="Search memories">
            <input id="mem-search" type="search" className={inputClass} value={q} onChange={(e) => setQ(e.target.value)} />
          </Field>
        </div>
        {!readOnly ? (
          <Button type="button" onClick={() => { setCreating(true); setSelected(null); }}>
            New memory
          </Button>
        ) : null}
      </div>
      {list.isPending ? <Loading /> : null}
      {list.isError ? <ErrorState error={list.error} onRetry={() => list.refetch()} /> : null}
      <div className="grid gap-6 md:layout-aside-lg">
        <div>
          {q.trim() ? (
            <>
              {search.isPending ? <Loading label="Searching…" /> : null}
              {search.isError ? <ErrorState error={search.error} /> : null}
              {search.data && search.data.items.length === 0 ? <EmptyState title="No matches" /> : null}
              {search.data?.items.length ? (
              <ItemList aria-label="Search results">
                {search.data.items.map((h) => (
                  <li key={h.key}>
                    <button type="button" className={memItem} onClick={() => { setSelected(h.key); setCreating(false); }}>
                      <span className="block truncate font-medium text-ink">
                        {h.title?.trim() || (items.find((m) => m.key === h.key) ? memoryName(items.find((m) => m.key === h.key)!) : "Memory")}
                      </span>
                      <span className="block text-xs text-muted">{h.snippet}</span>
                    </button>
                  </li>
                ))}
              </ItemList>
              ) : null}
            </>
          ) : (
            <>
              {list.data && items.length === 0 ? <EmptyState title="No memories yet" /> : null}
              {items.length ? (
              <ItemList aria-label="Memories">
                {items.map((m) => (
                  <li key={m.key}>
                    <button
                      type="button"
                      aria-current={selected === m.key}
                      className={cn(memItem, "aria-[current=true]:bg-subtle")}
                      onClick={() => { setSelected(m.key); setCreating(false); }}
                    >
                      <span className="block truncate font-medium text-ink">
                        {memoryName(m)}
                        {/* (P6-MEMLOCK, C-1150) The lock icon and badge only on locked rows. */}
                        {m.locked ? (
                          <Badge className="ml-2 align-middle">
                            <LockIcon className="size-3" aria-hidden="true" /> Locked
                          </Badge>
                        ) : null}
                      </span>
                      <span className="block truncate text-xs text-ink-2">{memoryPreview(m)}</span>
                      <span className="block text-xs text-muted">Updated <When value={m.updated_at} /></span>
                    </button>
                  </li>
                ))}
              </ItemList>
              ) : null}
            </>
          )}
        </div>
        <div>
          {readOnly && current ? <MemoryView memory={current} /> : null}
          {!readOnly && creating ? (
            <MemoryEditor key="new" agentId={agentId} agentName={agentName} taken={items.map((m) => m.key)} onDone={(k) => { setCreating(false); setSelected(k); }} onCancel={() => setCreating(false)} />
          ) : null}
          {!readOnly && !creating && current ? (
            <MemoryEditor key={`${current.key}:${current.revision}`} agentId={agentId} agentName={agentName} taken={[]} memory={current} onDone={(k) => setSelected(k)} onCancel={() => setSelected(null)} />
          ) : null}
          {!creating && !current ? <p className="text-sm text-muted">{readOnly ? "Select a memory to read it." : "Select a memory to edit it."}</p> : null}
        </div>
      </div>
    </div>
  );
}

/** A refusal because the key is taken (409): the key is Aurora's to make, so it tries the next one. */
function keyTaken(e: unknown): boolean {
  return e instanceof ApiError && e.status === 409;
}

/** Emergency read access: the memory's words, without any way to change them. */
function MemoryView({ memory }: { memory: Memory }) {
  const locker = personName(memory.locked_by?.display_name);
  return (
    <div className="space-y-4">
      <Card>
        <div className="space-y-4">
          {memory.title?.trim() ? (
            <div>
              <p className="text-xs text-muted">Title</p>
              <p className="text-sm font-medium text-ink">{memory.title}</p>
            </div>
          ) : null}
          <div>
            <p className="text-xs text-muted">What the agent remembers</p>
            <p className="whitespace-pre-wrap text-sm text-ink">{memory.content}</p>
          </div>
          <p className="text-xs text-muted">
            Updated <When value={memory.updated_at} />
          </p>
          {memory.locked && memory.locked_at ? (
            <p className="text-xs text-muted">
              {memory.locked_by ? `Locked by ${locker} on ${formatExact(memory.locked_at)}` : `Locked on ${formatExact(memory.locked_at)}`}
            </p>
          ) : null}
        </div>
      </Card>
      <Altitude id="memory.key" level="expert">
        <TechnicalDetails>
          <p>Memory key: {memory.key}</p>
        </TechnicalDetails>
      </Altitude>
    </div>
  );
}

/**
 * (P5-REFINE-12, behaviour-1) One box: what the agent should remember, an optional title, Save and Cancel. No key, no
 * description, no Markdown label: the key is made from the title (else the first words) and made unique here.
 */
function MemoryEditor({
  agentId,
  agentName,
  memory,
  taken,
  onDone,
  onCancel,
}: {
  agentId: string;
  agentName?: string;
  memory?: Memory;
  /** The agent's keys, so a new key is not one of them. */
  taken: string[];
  onDone: (key: string | null) => void;
  onCancel: () => void;
}) {
  const create = useCreateMemory(agentId);
  const put = usePutMemory(agentId, memory?.key ?? "");
  const del = useDeleteMemory(agentId);
  const lock = useSetMemoryLocked(agentId, memory?.key ?? "");
  const toast = useUndoToast();
  const [title, setTitle] = useState(memory?.title ?? "");
  const [content, setContent] = useState(memory?.content ?? "");
  const [lockIt, setLockIt] = useState(false);
  const [locked, setLocked] = useState(memory?.locked ?? false);
  const [history, setHistory] = useState(false);
  const [err, setErr] = useState("");
  const over = content.length > MEMORY_CONTENT_MAX;
  const failed = err ? null : (create.error ?? put.error ?? del.error ?? lock.error);
  const who = agentName ?? "your agent";
  // (P6-MEMLOCK, C-1150) The lock help names the agent; the same sentence stands at the checkbox.
  const lockHelp = `${agentName ?? "Your agent"} can read this memory but can't change or delete it. You can still edit it.`;
  // (P5-REFINE-20) A person's name is never shown raw: the locker goes through personName.
  const locker = personName(memory?.locked_by?.display_name);

  /** The switch saves on its own (not with Save); the toast names what happened. */
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

  async function submit(e: FormEvent) {
    e.preventDefault();
    if (!content.trim()) return setErr(`Write what ${who} should remember.`);
    if (over) return setErr(`That is too long: a memory holds up to ${MEMORY_CONTENT_MAX.toLocaleString("en")} characters.`);
    setErr("");
    const body = { title: title.trim() || null, description: memory?.description ?? null, content };
    try {
      if (memory) {
        await put.mutateAsync(body);
        onDone(memory.key);
        return;
      }
      // A taken key (a memory this list does not show yet) retries once with the next suffix.
      for (let attempt = 0; ; attempt++) {
        const key = newMemoryKey(title, content, taken, attempt);
        try {
          // (P6-MEMLOCK, C-1150) `locked` goes with the create only when the box is ticked.
          await create.mutateAsync({ key, ...body, ...(lockIt ? { locked: true } : {}) });
          onDone(key);
          return;
        } catch (error) {
          if (attempt >= 1 || !keyTaken(error)) throw error;
        }
      }
    } catch {
      // shown via failed
    }
  }

  return (
    <div className="space-y-4">
      <Card>
        <form onSubmit={(e) => void submit(e)} noValidate className="space-y-4">
          <Field
            id="mem-content"
            label={`What should ${who} remember?`}
            hint={`Plain words are fine. ${who} reads this before it answers.`}
            error={err || undefined}
          >
            <textarea id="mem-content" className={inputClass} rows={6} value={content} onChange={(e) => setContent(e.target.value)} />
          </Field>
          <SizeMeter label="Size" used={content.length} max={MEMORY_CONTENT_MAX} />
          <Field id="mem-title" label="Title (optional)">
            <input id="mem-title" className={inputClass} value={title} onChange={(e) => setTitle(e.target.value)} />
          </Field>
          {memory ? (
            <div>
              {/* (P6-MEMLOCK, C-1150) The Locked switch saves on its own, above Save; a lock is not
                  a revision, so the history below is unchanged. */}
              <SwitchField
                id="mem-locked"
                label="Locked"
                hint={lockHelp}
                checked={locked}
                onChange={(on) => void setLockedTo(on)}
                disabled={lock.isPending}
              />
              {locked && memory.locked_at ? (
                <p className="text-xs text-muted">
                  {memory.locked_by
                    ? `Locked by ${locker} on ${formatExact(memory.locked_at)}`
                    : `Locked on ${formatExact(memory.locked_at)}`}
                </p>
              ) : null}
            </div>
          ) : (
            <CheckboxField id="mem-lock-it" label="Lock it" hint={lockHelp} checked={lockIt} onChange={setLockIt} />
          )}
          <ErrorNotice error={failed} />
          <FormActions className="gap-2">
            <Button type="submit" disabled={over || create.isPending || put.isPending}>Save</Button>
            <Button type="button" variant="ghost" onClick={onCancel}>Cancel</Button>
            {memory ? (
              <>
                <Button type="button" variant="outline" onClick={() => setHistory((h) => !h)} aria-expanded={history}>
                  History
                </Button>
                <ConfirmAction
                  label="Delete"
                  title={`Delete the memory “${memoryName(memory)}”?`}
                  description="The agent forgets it, and its history goes with it. This can't be undone."
                  confirmLabel="Delete memory"
                  onConfirm={() => void del.mutateAsync(memory.key).then(() => onDone(null))}
                />
              </>
            ) : null}
          </FormActions>
        </form>
      </Card>
      {memory ? (
        <Altitude id="memory.key" level="expert">
          <TechnicalDetails>
            <p>Memory key: {memory.key}</p>
          </TechnicalDetails>
        </Altitude>
      ) : null}
      {memory && history ? <RevisionHistory agentId={agentId} memory={memory} /> : null}
    </div>
  );
}

export function RevisionHistory({ agentId, memory }: { agentId: string; memory: Memory }) {
  const revs = useMemoryRevisions(agentId, memory.key, true);
  const restore = useRestoreMemory(agentId, memory.key);
  const redact = useRedactRevision(agentId, memory.key);
  const [viewing, setViewing] = useState<number | null>(null);
  const [confirmRedact, setConfirmRedact] = useState<number | null>(null);
  const failed = restore.error ?? redact.error;

  return (
    <Card role="region" aria-label="Revision history" className="space-y-3">
      <h3 className="font-display text-base font-semibold text-ink">Revision history</h3>
      {revs.isPending ? <Loading /> : null}
      {revs.isError ? <ErrorState error={revs.error} onRetry={() => revs.refetch()} /> : null}
      <ErrorNotice error={failed} />
      <ul className="divide-y divide-line">
        {revs.data?.items.map((r) => (
          <li key={r.revision} className="space-y-1 py-3 text-sm first:pt-0 last:pb-0">
            <p>
              <strong>#{r.revision}</strong>
              {r.revision === memory.revision ? " (latest)" : ""} - {r.author_label} ({r.author_kind}, {r.provenance}) -{" "}
              <When value={r.created_at} />
            </p>
            {r.redacted ? <p className="text-muted">Content removed</p> : null}
            {viewing === r.revision && r.content != null ? (
              <pre className="mt-1 max-h-48 overflow-auto whitespace-pre-wrap rounded-control border border-line bg-subtle p-2 font-mono text-xs text-ink">{r.content}</pre>
            ) : null}
            <div className="mt-1 flex flex-wrap gap-2">
              {!r.redacted ? (
                <Button type="button" size="sm" variant="ghost" onClick={() => setViewing(viewing === r.revision ? null : r.revision)}>
                  {viewing === r.revision ? "Hide" : "View"} #{r.revision}
                </Button>
              ) : null}
              {!r.redacted && r.revision !== memory.revision ? (
                <Button type="button" size="sm" variant="outline" disabled={restore.isPending} onClick={() => restore.mutate(r.revision)}>
                  Restore #{r.revision}
                </Button>
              ) : null}
              {!r.redacted ? (
                confirmRedact === r.revision ? (
                  <>
                    <Button type="button" size="sm" variant="outline" onClick={() => void redact.mutateAsync(r.revision).then(() => setConfirmRedact(null))}>
                      Confirm removing content of #{r.revision}
                    </Button>
                    <Button type="button" size="sm" variant="ghost" onClick={() => setConfirmRedact(null)}>Cancel</Button>
                  </>
                ) : (
                  <Button
                    type="button"
                    size="sm"
                    variant="outline"
                    disabled={!canRedact(r, memory.revision)}
                    title={r.revision === memory.revision ? "The content of the latest revision cannot be removed. Save a new version first." : undefined}
                    onClick={() => setConfirmRedact(r.revision)}
                  >
                    Remove content of #{r.revision}
                  </Button>
                )
              ) : null}
            </div>
          </li>
        ))}
      </ul>
    </Card>
  );
}
