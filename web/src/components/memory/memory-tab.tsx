"use client";

import { useState, type FormEvent } from "react";

import { EmptyState, ErrorState, Loading } from "@/components/states";
import { Button } from "@/components/ui/button";
import { Field, inputClass } from "@/components/ui/field";
import { errorMessage } from "@/lib/api/errors";
import {
  MEMORY_CONTENT_MAX,
  MEMORY_KEY_MAX,
  MEMORY_KEY_RE,
  canRedact,
  useCreateMemory,
  useDeleteMemory,
  useMemories,
  useMemoryRevisions,
  useMemorySearch,
  usePutMemory,
  useRedactRevision,
  useRestoreMemory,
  type Memory,
} from "@/lib/api/memory";

import { SizeMeter } from "./size-meter";

function capitalize(s: string) {
  return s.charAt(0).toUpperCase() + s.slice(1);
}

export function MemoryTab({ agentId, memoryLanguage }: { agentId: string; memoryLanguage?: string }) {
  const list = useMemories(agentId);
  const [q, setQ] = useState("");
  const search = useMemorySearch(agentId, q);
  const [selected, setSelected] = useState<string | null>(null);
  const [creating, setCreating] = useState(false);
  const items = list.data?.items ?? [];
  const current = items.find((m) => m.key === selected);

  return (
    <div className="space-y-4">
      {memoryLanguage ? (
        <p role="note" className="rounded-md border border-neutral-200 bg-neutral-50 p-3 text-sm">
          Memories in this workspace are written in {capitalize(memoryLanguage)}.
        </p>
      ) : null}
      <div className="flex items-end gap-2">
        <div className="flex-1">
          <Field id="mem-search" label="Search memories">
            <input id="mem-search" type="search" className={inputClass} value={q} onChange={(e) => setQ(e.target.value)} />
          </Field>
        </div>
        <Button type="button" onClick={() => { setCreating(true); setSelected(null); }}>
          New memory
        </Button>
      </div>
      {list.isPending ? <Loading /> : null}
      {list.isError ? <ErrorState error={list.error} onRetry={() => list.refetch()} /> : null}
      <div className="grid gap-6 md:grid-cols-[18rem_1fr]">
        <div>
          {q.trim() ? (
            <>
              {search.isPending ? <Loading label="Searching…" /> : null}
              {search.isError ? <ErrorState error={search.error} /> : null}
              {search.data && search.data.items.length === 0 ? <EmptyState title="No matches" /> : null}
              <ul aria-label="Search results" className="space-y-1">
                {search.data?.items.map((h) => (
                  <li key={h.key}>
                    <button type="button" className="w-full rounded px-2 py-1.5 text-left text-sm hover:bg-neutral-50" onClick={() => { setSelected(h.key); setCreating(false); }}>
                      <span className="font-mono">{h.key}</span>
                      <span className="block text-xs text-neutral-600">{h.snippet}</span>
                    </button>
                  </li>
                ))}
              </ul>
            </>
          ) : (
            <>
              {list.data && items.length === 0 ? <EmptyState title="No memories yet" /> : null}
              <ul aria-label="Memories" className="space-y-1">
                {items.map((m) => (
                  <li key={m.key}>
                    <button
                      type="button"
                      aria-current={selected === m.key}
                      className="w-full rounded px-2 py-1.5 text-left text-sm hover:bg-neutral-50 aria-[current=true]:bg-neutral-100"
                      onClick={() => { setSelected(m.key); setCreating(false); }}
                    >
                      <span className="font-mono">{m.key}</span>
                      <span className="block truncate text-xs text-neutral-600">{m.index_line}</span>
                      <span className="block text-xs text-neutral-500">Updated {new Date(m.updated_at).toLocaleString()}</span>
                    </button>
                  </li>
                ))}
              </ul>
            </>
          )}
        </div>
        <div>
          {creating ? <MemoryEditor key="new" agentId={agentId} onDone={(k) => { setCreating(false); setSelected(k); }} /> : null}
          {!creating && current ? (
            <MemoryEditor key={`${current.key}:${current.revision}`} agentId={agentId} memory={current} onDone={(k) => setSelected(k)} />
          ) : null}
          {!creating && !current ? <p className="text-sm text-neutral-500">Select a memory to edit it.</p> : null}
        </div>
      </div>
    </div>
  );
}

export function keyIssue(key: string): string | null {
  if (!key || key.length > MEMORY_KEY_MAX || !MEMORY_KEY_RE.test(key))
    return `Use lowercase letters, digits and hyphens, at most ${MEMORY_KEY_MAX} characters.`;
  return null;
}

function MemoryEditor({ agentId, memory, onDone }: { agentId: string; memory?: Memory; onDone: (key: string | null) => void }) {
  const create = useCreateMemory(agentId);
  const put = usePutMemory(agentId, memory?.key ?? "");
  const del = useDeleteMemory(agentId);
  const [key, setKey] = useState(memory?.key ?? "");
  const [title, setTitle] = useState(memory?.title ?? "");
  const [description, setDescription] = useState(memory?.description ?? "");
  const [content, setContent] = useState(memory?.content ?? "");
  const [confirmDel, setConfirmDel] = useState(false);
  const [history, setHistory] = useState(false);
  const [err, setErr] = useState("");
  const over = content.length > MEMORY_CONTENT_MAX;
  const failed = create.error ?? put.error ?? del.error;

  async function submit(e: FormEvent) {
    e.preventDefault();
    const issue = memory ? null : keyIssue(key);
    if (issue) return setErr(issue);
    if (over) return setErr("The content is over the limit.");
    setErr("");
    const body = { title: title.trim() || null, description: description.trim() || null, content };
    try {
      if (memory) await put.mutateAsync(body);
      else await create.mutateAsync({ key, ...body });
      onDone(memory?.key ?? key);
    } catch {
      // shown via failed
    }
  }

  return (
    <div className="space-y-4">
      <form onSubmit={(e) => void submit(e)} noValidate className="space-y-3">
        <Field id="mem-key" label="Key" hint="Immutable once created." error={err && !memory ? err : undefined}>
          <input id="mem-key" className={inputClass} value={key} readOnly={!!memory} maxLength={MEMORY_KEY_MAX} onChange={(e) => setKey(e.target.value)} />
        </Field>
        <Field id="mem-title" label="Title">
          <input id="mem-title" className={inputClass} value={title} onChange={(e) => setTitle(e.target.value)} />
        </Field>
        <Field id="mem-desc" label="Description">
          <input id="mem-desc" className={inputClass} value={description} onChange={(e) => setDescription(e.target.value)} />
        </Field>
        <Field id="mem-content" label="Content (Markdown)">
          <textarea id="mem-content" className={`${inputClass} font-mono`} rows={12} value={content} onChange={(e) => setContent(e.target.value)} />
        </Field>
        <SizeMeter label="Content" used={content.length} max={MEMORY_CONTENT_MAX} />
        {err && memory ? <p role="alert" className="text-sm text-red-700">{err}</p> : null}
        {failed ? <p role="alert" className="text-sm text-red-700">{errorMessage(failed)}</p> : null}
        <div className="flex gap-2">
          <Button type="submit" disabled={over || create.isPending || put.isPending}>Save</Button>
          {memory ? (
            <>
              <Button type="button" variant="outline" onClick={() => setHistory((h) => !h)} aria-expanded={history}>
                History
              </Button>
              {confirmDel ? (
                <>
                  <Button type="button" variant="outline" onClick={() => void del.mutateAsync(memory.key).then(() => onDone(null))}>
                    Confirm delete
                  </Button>
                  <Button type="button" variant="ghost" onClick={() => setConfirmDel(false)}>Cancel</Button>
                </>
              ) : (
                <Button type="button" variant="outline" onClick={() => setConfirmDel(true)}>Delete</Button>
              )}
            </>
          ) : null}
        </div>
      </form>
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
    <section aria-label="Revision history" className="space-y-2 rounded-md border border-neutral-200 p-3">
      <h3 className="text-sm font-medium">Revision history</h3>
      {revs.isPending ? <Loading /> : null}
      {revs.isError ? <ErrorState error={revs.error} onRetry={() => revs.refetch()} /> : null}
      {failed ? <p role="alert" className="text-sm text-red-700">{errorMessage(failed)}</p> : null}
      <ul className="space-y-2">
        {revs.data?.items.map((r) => (
          <li key={r.revision} className="rounded border border-neutral-100 p-2 text-sm">
            <p>
              <strong>#{r.revision}</strong>
              {r.revision === memory.revision ? " (latest)" : ""} - {r.author_label} ({r.author_kind}, {r.provenance}) -{" "}
              {new Date(r.created_at).toLocaleString()}
            </p>
            {r.redacted ? <p className="text-neutral-500">Redacted</p> : null}
            {viewing === r.revision && r.content != null ? (
              <pre className="mt-1 max-h-48 overflow-auto whitespace-pre-wrap rounded bg-neutral-50 p-2 text-xs">{r.content}</pre>
            ) : null}
            <div className="mt-1 flex gap-2">
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
                      Confirm redact #{r.revision}
                    </Button>
                    <Button type="button" size="sm" variant="ghost" onClick={() => setConfirmRedact(null)}>Cancel</Button>
                  </>
                ) : (
                  <Button
                    type="button"
                    size="sm"
                    variant="outline"
                    disabled={!canRedact(r, memory.revision)}
                    title={r.revision === memory.revision ? "The latest revision cannot be redacted. Save a new version first." : undefined}
                    onClick={() => setConfirmRedact(r.revision)}
                  >
                    Redact #{r.revision}
                  </Button>
                )
              ) : null}
            </div>
          </li>
        ))}
      </ul>
    </section>
  );
}
