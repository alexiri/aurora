"use client";

import { Button } from "@/components/ui/button";
import { ChoiceChips } from "@/components/ui/choice-chips";
import { inputClass } from "@/components/ui/field";
import { HighlightedText } from "@/components/ui/highlighted-text";
import { ChevronDownIcon, LockIcon, SearchIcon, XIcon } from "@/components/ui/icons";
import { Menu, MenuContent, MenuItem, MenuTrigger } from "@/components/ui/menu";
import { useWhenNow } from "@/components/ui/when";
import { formatExact } from "@/lib/format-date";
import type { Memory, MemorySearchHit } from "@/lib/api/memory";
import { memoryName } from "@/lib/memory-key";
import { cn } from "@/lib/utils";

import { addedByMe, sourceLine } from "./memory-words";

export type MemoryFilter = "all" | "mine" | "agent" | "locked";
export type MemorySort = "recent" | "name";

const SORT_LABEL: Record<MemorySort, string> = { recent: "Recently changed", name: "Name" };

export function matchesFilter(m: Memory, filter: MemoryFilter, me: string | null | undefined): boolean {
  if (filter === "mine") return addedByMe(m.added_by, me);
  if (filter === "agent") return m.added_by.kind === "agent";
  if (filter === "locked") return m.locked;
  return true;
}

export function sortMemories(items: Memory[], sort: MemorySort): Memory[] {
  const byName = (a: Memory, b: Memory) => memoryName(a).localeCompare(memoryName(b));
  return [...items].sort(sort === "name" ? byName : (a, b) => Date.parse(b.updated_at) - Date.parse(a.updated_at) || byName(a, b));
}

export function FilterChips({
  items,
  filter,
  onFilter,
  agentName,
  me,
}: {
  items: Memory[];
  filter: MemoryFilter;
  onFilter: (f: MemoryFilter) => void;
  agentName: string;
  me: string | null | undefined;
}) {
  const count = (f: MemoryFilter) => items.filter((m) => matchesFilter(m, f, me)).length;
  const options: { value: MemoryFilter; label: string }[] = [
    { value: "all", label: `All ${count("all")}` },
    { value: "mine", label: `By you ${count("mine")}` },
    { value: "agent", label: `By ${agentName} ${count("agent")}` },
    { value: "locked", label: `Locked ${count("locked")}` },
  ];
  return <ChoiceChips label="Show" value={filter} options={options} onChange={onFilter} />;
}

export function SortControl({ sort, onSort }: { sort: MemorySort; onSort: (s: MemorySort) => void }) {
  return (
    <div className="flex items-center justify-between gap-2 text-xs text-muted">
      <span>Sorted by</span>
      <Menu>
        <MenuTrigger asChild>
          <button type="button" className="inline-flex min-h-9 items-center gap-1 rounded-control px-1 font-semibold text-ink-2 hover:text-ink" aria-label={`Sorted by ${SORT_LABEL[sort]}`}>
            {SORT_LABEL[sort]}
            <ChevronDownIcon className="size-3.5" />
          </button>
        </MenuTrigger>
        <MenuContent align="end">
          {(Object.keys(SORT_LABEL) as MemorySort[]).map((s) => (
            <MenuItem key={s} aria-current={s === sort ? "true" : undefined} className={s === sort ? "font-semibold" : undefined} onSelect={() => onSort(s)}>
              {SORT_LABEL[s]}
            </MenuItem>
          ))}
        </MenuContent>
      </Menu>
    </div>
  );
}

export function SearchField({ value, onChange }: { value: string; onChange: (v: string) => void }) {
  return (
    <div className="relative">
      <label htmlFor="mem-search" className="sr-only">
        Search memories
      </label>
      <SearchIcon className="pointer-events-none absolute left-3 top-1/2 size-4 -translate-y-1/2 text-muted" />
      <input
        id="mem-search"
        type="search"
        placeholder="Search memories"
        autoComplete="off"
        className={cn(inputClass, "min-h-11 pl-9 pr-10 [&::-webkit-search-cancel-button]:hidden")}
        value={value}
        onChange={(e) => onChange(e.target.value)}
      />
      {value ? (
        <button
          type="button"
          aria-label="Clear the search field"
          onClick={() => onChange("")}
          className="absolute right-1 top-1/2 inline-flex size-9 -translate-y-1/2 items-center justify-center rounded-control text-muted hover:text-ink"
        >
          <XIcon className="size-4" />
        </button>
      ) : null}
    </div>
  );
}

/** "1 memory matches “rotation”" with **Clear**, above the results. */
export function SearchSummary({ q, count, onClear }: { q: string; count: number; onClear: () => void }) {
  const words = count === 1 ? "1 memory matches" : count === 0 ? "No memories match" : `${count} memories match`;
  return (
    <div className="flex items-center justify-between gap-3 text-sm text-ink">
      <p role="status" className="min-w-0 break-words">
        {words} “{q}”
      </p>
      <Button type="button" variant="ghost" size="sm" className="shrink-0 text-accent-ink underline underline-offset-2" onClick={onClear}>
        Clear
      </Button>
    </div>
  );
}

const rowClass =
  "block w-full min-w-0 row-rail border-l-transparent px-3 py-2.5 text-left hover:bg-subtle aria-[current=true]:border-l-accent aria-[current=true]:bg-subtle";
const listClass = "divide-y divide-line overflow-hidden rounded-card border border-line bg-surface";

function Source({ memory, agentName, me, now }: { memory: Memory; agentName: string; me: string | null | undefined; now: number }) {
  return (
    <span className="block break-words text-xs text-muted" title={formatExact(memory.created_at)}>
      {sourceLine(memory, agentName, me, now)}
    </span>
  );
}

export function MemoryRows({
  items,
  current,
  dirtyKey,
  agentName,
  me,
  onChoose,
}: {
  items: Memory[];
  current: string | null;
  /** The memory with unsaved changes, marked with a dot. */
  dirtyKey: string | null;
  agentName: string;
  me: string | null | undefined;
  onChoose: (key: string) => void;
}) {
  const now = useWhenNow();
  return (
    <ul aria-label="Memories" className={listClass}>
      {items.map((m) => (
        <li key={m.key}>
          <button type="button" aria-current={current === m.key} className={rowClass} onClick={() => onChoose(m.key)}>
            <span className="flex min-w-0 items-start justify-between gap-2">
              <span className="min-w-0 break-words text-sm font-semibold text-ink">{memoryName(m)}</span>
              <span className="flex shrink-0 items-center gap-1.5 pt-0.5">
                {dirtyKey === m.key ? (
                  <span className="inline-flex text-pending-ink" title="Unsaved changes" data-testid="dirty-dot">
                    <span aria-hidden="true" className="size-2 rounded-full bg-current" />
                    <span className="sr-only">Unsaved changes</span>
                  </span>
                ) : null}
                {m.locked ? (
                  <span className="inline-flex text-muted" title="Locked">
                    <LockIcon className="size-3.5" />
                    <span className="sr-only">Locked</span>
                  </span>
                ) : null}
              </span>
            </span>
            {m.description?.trim() ? (
              <span className="block truncate text-xs text-ink-2">{m.description.trim()}</span>
            ) : (
              <span className="block text-xs italic text-muted">No description</span>
            )}
            <Source memory={m} agentName={agentName} me={me} now={now} />
          </button>
        </li>
      ))}
    </ul>
  );
}

export function SearchRows({
  hits,
  items,
  current,
  agentName,
  me,
  onChoose,
}: {
  hits: MemorySearchHit[];
  /** The listed memories, for a hit's name and source line. */
  items: Memory[];
  current: string | null;
  agentName: string;
  me: string | null | undefined;
  onChoose: (key: string) => void;
}) {
  const now = useWhenNow();
  return (
    <ul aria-label="Search results" className={listClass}>
      {hits.map((h) => {
        const m = items.find((x) => x.key === h.key);
        // `title_highlights` index the title, or the key when there is no title (C-1171). A memory without a title is
        // named by its first line here, as in the list, so the key's ranges mark nothing.
        const title = h.title?.trim() ? h.title : null;
        return (
          <li key={h.key}>
            <button type="button" aria-current={current === h.key} className={rowClass} onClick={() => onChoose(h.key)}>
              <span className="block break-words text-sm font-semibold text-ink">
                {title ? <HighlightedText text={title} highlights={h.title_highlights} tone="solid" /> : m ? memoryName(m) : h.key}
              </span>
              <span className="line-clamp-3 break-words text-xs text-ink-2">
                <HighlightedText text={h.snippet} highlights={h.highlights} tone="solid" />
              </span>
              {m ? <Source memory={m} agentName={agentName} me={me} now={now} /> : null}
            </button>
          </li>
        );
      })}
    </ul>
  );
}
