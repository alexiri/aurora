"use client";

import { useCallback, useRef, useState } from "react";

import { errorMessage } from "@/lib/api/errors";

/**
 * (P5-REFINE-19, `bulk`, Expert) A bulk action is the per-item route called once per item, **in order**, with a progress
 * line ("2 of 3"). A failure is kept by name and the rest go on; what succeeded is not rolled back; the failed ones can be
 * tried again. (There is no bulk route, so the same checks, the same audit rows and the same rate limits apply.)
 */

export interface BulkTarget {
  id: string;
  /** What the person calls it: shown in the failure list. */
  name: string;
}

export interface BulkFailure extends BulkTarget {
  message: string;
}

export interface BulkOutcome {
  total: number;
  done: number;
  failed: BulkFailure[];
}

/** Runs `action` for each target in turn. Never throws: a failure becomes an entry of `failed`. */
export async function runBulk(targets: BulkTarget[], action: (t: BulkTarget) => Promise<unknown>, onProgress?: (done: number, total: number) => void): Promise<BulkOutcome> {
  const failed: BulkFailure[] = [];
  let done = 0;
  for (const t of targets) {
    try {
      await action(t);
      done++;
    } catch (e) {
      failed.push({ ...t, message: errorMessage(e) });
    }
    onProgress?.(done + failed.length, targets.length);
  }
  return { total: targets.length, done, failed };
}

export interface BulkState {
  running: boolean;
  /** The line while it runs: "2 of 3"; after it ends, "3 of 3 done". */
  progress: { done: number; total: number } | null;
  /** Finished with these failures (empty: all went through). */
  outcome: BulkOutcome | null;
}

/** State and controls for one bulk action at a time on a list. */
export function useBulk() {
  const [state, setState] = useState<BulkState>({ running: false, progress: null, outcome: null });
  const last = useRef<{ targets: BulkTarget[]; action: (t: BulkTarget) => Promise<unknown> } | null>(null);

  const run = useCallback(async (targets: BulkTarget[], action: (t: BulkTarget) => Promise<unknown>) => {
    last.current = { targets, action };
    setState({ running: true, progress: { done: 0, total: targets.length }, outcome: null });
    const outcome = await runBulk(targets, action, (done, total) => setState((s) => ({ ...s, progress: { done, total } })));
    setState({ running: false, progress: { done: outcome.total, total: outcome.total }, outcome });
    return outcome;
  }, []);

  /** Again, for the ones that failed. */
  const retry = useCallback(async () => {
    const l = last.current;
    if (!l) return null;
    const failedIds = new Set((state.outcome?.failed ?? []).map((f) => f.id));
    return run(
      l.targets.filter((t) => failedIds.has(t.id)),
      l.action,
    );
  }, [run, state.outcome]);

  const reset = useCallback(() => setState({ running: false, progress: null, outcome: null }), []);
  return { ...state, run, retry, reset };
}
