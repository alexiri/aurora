import { EVAL_RUNS_PER_AGENT_DAY, type EvalRun } from "@/lib/api/evals";

export const isActive = (r: Pick<EvalRun, "status">) => r.status === "queued" || r.status === "running";

/** UTC calendar day of an instant, `2026-11-17`. */
const utcDay = (d: Date | string) => new Date(d).toISOString().slice(0, 10);

/**
 * Why "Start run" is disabled, or null when it is not. The API decides in the end (409 `eval_run_active`, 429
 * `eval_runs_limited`, 423 `frozen`, 422 `eval_no_questions`); this only keeps a person from a click that is certain to be
 * refused, and says why. The day's count covers the runs the list has loaded.
 */
export function startBlockedReason(
  runs: EvalRun[],
  opts: { enabledQuestions: number; frozen: boolean; now?: Date },
): string | null {
  if (opts.frozen) return "This agent is paused, so no run can start.";
  if (opts.enabledQuestions === 0) return "Add at least one question that is included in runs.";
  if (runs.some(isActive)) return "A run is in progress. Wait for it to finish or cancel it.";
  const today = utcDay(opts.now ?? new Date());
  const used = runs.filter((r) => utcDay(r.created_at) === today).length;
  if (used >= EVAL_RUNS_PER_AGENT_DAY) {
    return `This agent has used its ${EVAL_RUNS_PER_AGENT_DAY} runs for today, scheduled runs included. The count restarts at midnight UTC.`;
  }
  return null;
}

/** Share of the run's questions that have an outcome (the run's progress while it is active). */
export function progressRatio(r: Pick<EvalRun, "counts">): number {
  const c = r.counts;
  return c.total > 0 ? (c.passed + c.failed + c.errored + c.not_scored) / c.total : 0;
}

/** `85` → `85%`, `66.666` → `66.7%`; null when nothing was scored. */
export const fmtRate = (rate: number | null | undefined): string =>
  rate == null ? "—" : `${Number.isInteger(rate) ? rate : rate.toFixed(1)}%`;
