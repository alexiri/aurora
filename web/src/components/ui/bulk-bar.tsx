"use client";

import { useCallback, useState, type ReactNode } from "react";

import { Altitude } from "@/components/ui/altitude";
import { Button } from "@/components/ui/button";
import { Notice } from "@/components/ui/notice";
import { useAltitude } from "@/lib/altitude";
import type { BulkState } from "@/lib/bulk";

/** Bulk actions exist from the Expert altitude (`bulk`): the checkboxes and the bar. Same rule as `<Altitude level="expert">`. */
export function useBulkOn(): boolean {
  return useAltitude().allows("expert");
}

/**
 * The selection of a list: the ids that are ticked, limited to the ids still in the list (a row that went away is not
 * selected any more). `selectable` is every id that may be ticked.
 */
export function useSelection(selectable: string[]) {
  const [picked, setPicked] = useState<Set<string>>(new Set());
  // Only what is still on the list and tickable.
  const selected = new Set(selectable.filter((id) => picked.has(id)));
  const toggle = useCallback((id: string, on: boolean) => {
    setPicked((p) => {
      const next = new Set(p);
      if (on) next.add(id);
      else next.delete(id);
      return next;
    });
  }, []);
  const setAll = useCallback((on: boolean) => setPicked(on ? new Set(selectable) : new Set()), [selectable]);
  const clear = useCallback(() => setPicked(new Set()), []);
  return { selected, toggle, setAll, clear, all: selectable.length > 0 && selected.size === selectable.length, some: selected.size > 0 && selected.size < selectable.length };
}

/** A row's checkbox, named "Select <name>". Not shown below Expert. */
export function BulkCheckbox({ name, checked, onChange, disabled, title }: { name: string; checked: boolean; onChange: (on: boolean) => void; disabled?: boolean; title?: string }) {
  return (
    <Altitude id="bulk" level="expert">
      <input
        type="checkbox"
        aria-label={`Select ${name}`}
        title={title}
        checked={checked}
        disabled={disabled}
        onChange={(e) => onChange(e.target.checked)}
        className="size-4 shrink-0 accent-accent pointer-coarse:size-5"
      />
    </Altitude>
  );
}

/** The header's checkbox: all, some (indeterminate) or none of the rows that may be ticked. */
export function BulkSelectAll({ label, all, some, disabled, onChange }: { label: string; all: boolean; some: boolean; disabled?: boolean; onChange: (on: boolean) => void }) {
  return (
    <Altitude id="bulk" level="expert">
      <input
        type="checkbox"
        aria-label={label}
        checked={all}
        disabled={disabled}
        ref={(el) => {
          if (el) el.indeterminate = some;
        }}
        onChange={(e) => onChange(e.target.checked)}
        className="mr-2 size-4 shrink-0 accent-accent align-middle pointer-coarse:size-5"
      />
    </Altitude>
  );
}

/**
 * (P5-REFINE-19, `bulk`) "<n> selected" with the list's actions as children, a progress line while they run ("2 of 3"), and,
 * when some failed, their names with **Retry**. Shown only while something is selected or a result is waiting.
 */
export function BulkBar({ count, noun = "selected", bulk, onClear, children }: { count: number; noun?: string; bulk: Pick<BulkState, "running" | "progress" | "outcome"> & { retry: () => void; reset: () => void }; onClear: () => void; children: ReactNode }) {
  const failed = bulk.outcome?.failed ?? [];
  if (count === 0 && !bulk.outcome && !bulk.running) return null;
  return (
    <Altitude id="bulk" level="expert">
      <div role="region" aria-label="Bulk actions" data-testid="bulk-bar" className="space-y-2 rounded-card border border-line bg-subtle px-3 py-2">
        <div className="flex flex-wrap items-center gap-x-3 gap-y-2">
          <span className="text-sm font-medium text-ink">
            {count} {noun}
          </span>
          {children}
          <Button type="button" size="sm" variant="ghost" disabled={bulk.running} onClick={onClear}>
            Clear selection
          </Button>
          {bulk.progress ? (
            <span role="status" data-testid="bulk-progress" className="text-sm text-ink-2">
              {bulk.running ? `${bulk.progress.done} of ${bulk.progress.total}` : `${bulk.progress.done} of ${bulk.progress.total} done`}
            </span>
          ) : null}
        </div>
        {!bulk.running && bulk.outcome && failed.length > 0 ? (
          <Notice tone="danger" role="alert" className="space-y-2">
            <p className="font-medium">
              {failed.length === 1 ? "1 was not changed" : `${failed.length} were not changed`}. The others were.
            </p>
            <ul className="list-disc pl-5">
              {failed.map((f) => (
                <li key={f.id}>
                  {f.name}: {f.message}
                </li>
              ))}
            </ul>
            <div className="flex flex-wrap gap-2">
              <Button type="button" size="sm" variant="outline" onClick={bulk.retry}>
                Retry {failed.length === 1 ? failed[0]!.name : `the ${failed.length} that failed`}
              </Button>
              <Button type="button" size="sm" variant="ghost" onClick={bulk.reset}>
                Dismiss
              </Button>
            </div>
          </Notice>
        ) : null}
        {!bulk.running && bulk.outcome && failed.length === 0 ? (
          <p role="status" className="text-xs text-ink-2">
            Done.{" "}
            <button type="button" className="underline underline-offset-2" onClick={bulk.reset}>
              Dismiss
            </button>
          </p>
        ) : null}
      </div>
    </Altitude>
  );
}
