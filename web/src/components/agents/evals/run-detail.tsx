"use client";

import { useState } from "react";

import { fmtDate } from "@/components/account/key-table";
import { EmptyState, ErrorState, Loading } from "@/components/states";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { ConfirmDialog } from "@/components/ui/confirm-dialog";
import { DataTable, Td, Th } from "@/components/ui/data-table";
import { Field, inputClass } from "@/components/ui/field";
import { Notice } from "@/components/ui/notice";
import { ItemList, ListRow, Section } from "@/components/ui/section";
import { TechnicalDetails } from "@/components/ui/technical-details";
import { formatExact, formatWhen } from "@/lib/format-date";
import {
  useCancelEvalRun,
  useDeleteEvalRun,
  useEvalRun,
  useEvalRuns,
  useStartEvalRun,
  type EvalResult,
  type EvalRunDetail,
} from "@/lib/api/evals";

import { OUTCOME, RUN_REASON, RUN_STATUS, evalErrorMessage, reasonText } from "./copy";
import { diffFingerprints } from "./fingerprint-diff";
import { HashValue } from "./hash-value";
import { RegressionNotice, StatusBadge } from "./runs-section";
import { fmtRate, isActive } from "./run-state";

const excerpt = (s: string | null | undefined, n = 160) => {
  if (!s) return "";
  const one = s.replace(/\s+/g, " ").trim();
  return one.length > n ? `${one.slice(0, n)}…` : one;
};

function OutcomeBadge({ outcome }: { outcome: string }) {
  const o = OUTCOME[outcome] ?? { label: outcome, tone: "neutral" as const };
  return <Badge tone={o.tone}>{o.label}</Badge>;
}

/** Why a result is what it is: the reason in words, the judge's rationale, the expected answer. */
function why(r: EvalResult, agentName: string): string {
  const parts: string[] = [];
  if (r.reason_code) parts.push(reasonText(r.reason_code, agentName));
  // (behaviour-3) What happened, in numbers: "Matches 42" or `Answer was "pong", expected 42`.
  else if (r.expected_kind !== "judged" && r.extracted_value) {
    const want = expectedLine(r);
    parts.push(r.outcome === "pass" ? `Matches ${want}` : r.outcome === "fail" ? `Answer was "${r.extracted_value}", expected ${want}` : "");
  }
  if (r.judge?.rationale) parts.push(`Grader: ${r.judge.rationale}`);
  return parts.filter(Boolean).join(" ");
}

function expectedLine(r: EvalResult): string {
  const e = r.expected;
  if (!e) return "";
  if (r.expected_kind === "numeric") return `${e.number ?? "?"}${e.tolerance ? ` ± ${e.tolerance}` : ""}`;
  if (r.expected_kind === "text") return e.text ?? "";
  return e.criteria ?? "";
}

function ResultDetails({ r }: { r: EvalResult }) {
  return (
    <details className="text-xs text-ink-2">
      <summary className="cursor-pointer font-medium text-ink">Details</summary>
      <dl className="mt-1.5 space-y-1.5">
        {r.refused_writes > 0 ? (
          <div>
            <dt className="font-medium">Refused writes</dt>
            <dd>{r.refused_writes}</dd>
          </div>
        ) : null}
        <div>
          <dt className="font-medium">Expected</dt>
          <dd className="break-words">{expectedLine(r) || "—"}</dd>
        </div>
        <div>
          <dt className="font-medium">Answer</dt>
          <dd className="whitespace-pre-wrap break-words">{r.answer || "—"}</dd>
        </div>
        {r.extracted_value ? (
          <div>
            <dt className="font-medium">Read from the ANSWER: line</dt>
            <dd className="break-words">{r.extracted_value}</dd>
          </div>
        ) : null}
      </dl>
    </details>
  );
}

/** (behaviour-4) A result row's own Re-run: a new run of just that question. Off, with the reason, while a run is active. */
function RerunButton({ r, rerun }: { r: EvalResult; rerun: Rerun }) {
  if (!r.question.id) return null;
  const id = r.question.id;
  return (
    <Button
      size="sm"
      variant="outline"
      disabled={rerun.blocked !== null || rerun.busy}
      title={rerun.blocked ?? undefined}
      aria-label={`Re-run question ${r.position}`}
      onClick={() => rerun.start(id)}
    >
      Re-run
    </Button>
  );
}

interface Rerun {
  /** Why a re-run cannot start now (a run is active), else null. */
  blocked: string | null;
  busy: boolean;
  start: (questionId: string) => void;
}

function Results({ run, rerun }: { run: EvalRunDetail; rerun: Rerun }) {
  const agentName = run.agent.name;
  const active = isActive(run);
  if (run.results.length === 0) return <EmptyState title="No results yet">Results appear here as each question finishes.</EmptyState>;
  // A row that has not finished while the run is active says what it waits for.
  const pending = (r: EvalResult) => active && !r.finished_at;
  return (
    <>
      <div className="hidden md:block">
        <DataTable caption="Results" className="min-w-0">
          <thead>
            <tr>
              <Th>Question</Th>
              <Th>Answer</Th>
              <Th>Why</Th>
              <Th>
                <span className="sr-only">Actions</span>
              </Th>
            </tr>
          </thead>
          <tbody>
            {run.results.map((r) => (
              <tr key={r.position}>
                <Td className="max-w-xs whitespace-normal break-words">
                  <span className="mr-2 tabular-nums text-ink-2">{r.position}.</span>
                  {excerpt(r.question.text, 120)}
                </Td>
                <Td className="max-w-sm whitespace-normal break-words text-ink-2">
                  {pending(r) ? <span>Running…</span> : <OutcomeBadge outcome={r.outcome} />}
                  <span className="mt-1 block">{excerpt(r.answer) || "—"}</span>
                  <ResultDetails r={r} />
                </Td>
                <Td className="max-w-xs whitespace-normal break-words text-ink-2">{pending(r) ? "—" : why(r, agentName) || "—"}</Td>
                <Td className="text-right">
                  <RerunButton r={r} rerun={rerun} />
                </Td>
              </tr>
            ))}
          </tbody>
        </DataTable>
      </div>
      <ItemList aria-label="Results" className="md:hidden">
        {run.results.map((r) => (
          <ListRow key={r.position} actions={<RerunButton r={r} rerun={rerun} />}>
            <div className="min-w-0 flex-1 space-y-1.5 text-sm">
              <p className="flex flex-wrap items-center gap-2">
                <span className="tabular-nums text-ink-2">{r.position}.</span>
                {pending(r) ? <span className="text-xs text-ink-2">Running…</span> : <OutcomeBadge outcome={r.outcome} />}
              </p>
              <p className="break-words text-ink">{excerpt(r.question.text, 120)}</p>
              {r.answer ? <p className="break-words text-xs text-ink-2">{excerpt(r.answer)}</p> : null}
              {!pending(r) && why(r, agentName) ? <p className="break-words text-xs text-ink-2">{why(r, agentName)}</p> : null}
              <ResultDetails r={r} />
            </div>
          </ListRow>
        ))}
      </ItemList>
    </>
  );
}

/** The configuration difference against another run (D-291): exactly the fields that changed. */
function Compare({ agentId, run, preferred }: { agentId: string; run: EvalRunDetail; preferred?: string }) {
  const runsQ = useEvalRuns(agentId);
  const all = runsQ.data?.pages.flatMap((p) => p.items) ?? [];
  const others = all.filter((r) => r.id !== run.id);
  // The previous run: the next older one in the list (it is newest first).
  const index = all.findIndex((r) => r.id === run.id);
  const previous = (index >= 0 ? all[index + 1] : undefined) ?? others[0];
  const [chosen, setChosen] = useState<string | undefined>(preferred);
  const otherId = chosen ?? preferred ?? previous?.id ?? null;
  const other = useEvalRun(agentId, otherId);
  return (
    <Section id="eval-compare" title="What changed" description="The agent's configuration as each run recorded it. The difference is a list of facts, not a diagnosis.">
      {others.length === 0 ? (
        <p className="text-sm text-ink-2">There is no other run to compare with yet.</p>
      ) : (
        <>
          <Field id="evc-with" label="Compare with">
            <select id="evc-with" className={`${inputClass} sm:max-w-md`} value={otherId ?? ""} onChange={(e) => setChosen(e.target.value)}>
              {others.map((r) => (
                <option key={r.id} value={r.id}>
                  {fmtDate(r.started_at ?? r.created_at)} · {RUN_STATUS[r.status]?.label ?? r.status} · {fmtRate(r.pass_rate)}
                </option>
              ))}
            </select>
          </Field>
          {other.isPending ? <Loading /> : null}
          {other.isError ? <ErrorState error={other.error} onRetry={() => void other.refetch()} /> : null}
          {other.data ? <Diff earlier={other.data} later={run} /> : null}
        </>
      )}
    </Section>
  );
}

function Diff({ earlier, later }: { earlier: EvalRunDetail; later: EvalRunDetail }) {
  // Older run first, whichever way round the two were chosen.
  const [a, b] = new Date(earlier.created_at) <= new Date(later.created_at) ? [earlier, later] : [later, earlier];
  const changes = diffFingerprints(a.fingerprint, b.fingerprint);
  if (changes.length === 0) {
    return (
      <Notice tone="neutral" role="status">
        The configuration is the same in both runs.
      </Notice>
    );
  }
  return (
    <>
      <div className="hidden md:block">
        <DataTable caption="Changed configuration fields" className="min-w-0">
          <thead>
            <tr>
              <Th>Field</Th>
              <Th>Earlier run</Th>
              <Th>Later run</Th>
            </tr>
          </thead>
          <tbody>
            {changes.map((c) => (
              <tr key={c.key}>
                <Td className="whitespace-normal break-words font-medium">{c.field}</Td>
                <Td className="whitespace-normal break-words text-ink-2">
                  <HashValue value={c.before} label={`${c.field}, earlier run`} />
                </Td>
                <Td className="whitespace-normal break-words">
                  <HashValue value={c.after} label={`${c.field}, later run`} />
                </Td>
              </tr>
            ))}
          </tbody>
        </DataTable>
      </div>
      <ItemList aria-label="Changed configuration fields" className="md:hidden">
        {changes.map((c) => (
          <ListRow key={c.key}>
            <div className="min-w-0 flex-1 space-y-1 text-sm">
              <p className="break-words font-medium text-ink">{c.field}</p>
              <p className="break-words text-xs text-ink-2">
                Earlier: <HashValue value={c.before} label={`${c.field}, earlier run`} />
              </p>
              <p className="break-words text-xs text-ink">
                Later: <HashValue value={c.after} label={`${c.field}, later run`} />
              </p>
            </div>
          </ListRow>
        ))}
      </ItemList>
    </>
  );
}

export function RunDetail({
  agentId,
  runId,
  compareWith,
  canOwn,
  onBack,
  onOpenRun,
}: {
  agentId: string;
  runId: string;
  compareWith?: string;
  canOwn: boolean;
  onBack: () => void;
  /** Opens the run a Re-run started. */
  onOpenRun?: (runId: string) => void;
}) {
  const q = useEvalRun(agentId, runId);
  const cancel = useCancelEvalRun(agentId);
  const del = useDeleteEvalRun(agentId);
  const start = useStartEvalRun(agentId);
  const [confirm, setConfirm] = useState<"cancel" | "delete" | null>(null);
  const back = (
    <Button size="sm" variant="outline" onClick={onBack}>
      All test runs
    </Button>
  );
  if (q.isPending) return <Loading />;
  if (q.isError) {
    return (
      <div className="space-y-3">
        {back}
        <ErrorState error={q.error} onRetry={() => void q.refetch()} />
      </div>
    );
  }
  const run = q.data;
  const c = run.counts;
  const err = cancel.error ?? del.error ?? start.error;
  const startedBy = run.trigger === "schedule" ? `the schedule ${run.schedule?.name ?? ""}`.trim() : (run.started_by?.label ?? "someone who has left");
  const done = c.passed + c.failed + c.errored + c.not_scored;
  const rerun: Rerun = {
    blocked: isActive(run) ? "A run is in progress. Wait for it to finish or cancel it." : null,
    busy: start.isPending,
    start: (id) => start.mutate([id], { onSuccess: (r) => onOpenRun?.(r.id) }),
  };
  return (
    <div className="space-y-6">
      <Section
        id="eval-run"
        title={
          <span className="flex flex-wrap items-center gap-2">
            Test run, {formatExact(run.started_at ?? run.created_at)} <StatusBadge status={run.status} />
          </span>
        }
        description={
          isActive(run)
            ? `Running · ${done} of ${c.total} done · started by ${startedBy}`
            : `Started by ${startedBy}. ${run.finished_at ? `Finished ${formatWhen(run.finished_at).toLowerCase()}.` : ""}`
        }
        actions={
          <>
            {back}
            {isActive(run) ? (
              <Button size="sm" variant="outline" onClick={() => setConfirm("cancel")}>
                Cancel run
              </Button>
            ) : null}
            {canOwn && !isActive(run) ? (
              <Button size="sm" variant="danger" onClick={() => setConfirm("delete")}>
                Delete run
              </Button>
            ) : null}
          </>
        }
      >
        {err ? (
          <Notice tone="danger" role="alert">
            {evalErrorMessage(err)}
          </Notice>
        ) : null}
        {run.error_code ? (
          <Notice tone="pending" role="status">
            {RUN_REASON[run.error_code] ?? `The run stopped: ${run.error_code.replace(/_/g, " ")}.`}
          </Notice>
        ) : null}
        <RegressionNotice run={run} />
        <dl className="grid grid-cols-2 gap-3 rounded-card border border-line bg-surface p-4 text-sm shadow-card sm:grid-cols-3 lg:grid-cols-6">
          {[
            ["Pass rate", fmtRate(run.pass_rate)],
            ["Passed", c.passed],
            ["Failed", c.failed],
            ["Couldn't be checked", c.errored + c.not_scored],
          ].map(([k, v]) => (
            <div key={k}>
              <dt className="text-xs text-muted">{k}</dt>
              <dd className="text-base font-semibold tabular-nums text-ink">{v}</dd>
            </div>
          ))}
        </dl>
        <details className="text-sm text-ink-2">
          <summary className="cursor-pointer font-medium text-ink">Details</summary>
          <dl className="mt-2 space-y-1">
            <div className="flex gap-2">
              <dt className="text-muted">Refused writes</dt>
              <dd className="tabular-nums">{c.refused_writes}</dd>
            </div>
            {run.judge_usage ? (
              <div className="flex gap-2">
                <dt className="text-muted">Grader usage</dt>
                <dd>
                  {run.judge_usage.input_tokens} input tokens, {run.judge_usage.output_tokens} output tokens
                </dd>
              </div>
            ) : null}
          </dl>
        </details>
        <TechnicalDetails>
          <p className="flex flex-wrap items-center gap-x-2">
            Configuration: <HashValue value={run.fingerprint_hash} label="configuration hash" />
          </p>
        </TechnicalDetails>
      </Section>
      <Section id="eval-results" title="Results" description="Answers and expected answers are visible to the people who can use this agent.">
        {start.error ? (
          <Notice tone="danger" role="alert">
            {evalErrorMessage(start.error)}
          </Notice>
        ) : null}
        <Results run={run} rerun={rerun} />
      </Section>
      <Compare key={run.id} agentId={agentId} run={run} preferred={compareWith} />
      <ConfirmDialog
        open={confirm === "cancel"}
        onOpenChange={(o) => {
          if (!o) setConfirm(null);
        }}
        title="Cancel this run?"
        description="The question in progress is stopped and the rest are not scored. Results already recorded are kept."
        confirmLabel="Stop the run"
        danger
        busy={cancel.isPending}
        onConfirm={() => cancel.mutate(run.id, { onSettled: () => setConfirm(null) })}
      />
      <ConfirmDialog
        open={confirm === "delete"}
        onOpenChange={(o) => {
          if (!o) setConfirm(null);
        }}
        title="Delete this run?"
        description="The run, its results and its conversations are removed for good."
        confirmLabel="Delete run"
        danger
        busy={del.isPending}
        onConfirm={() => del.mutate(run.id, { onSuccess: onBack, onSettled: () => setConfirm(null) })}
      />
    </div>
  );
}
