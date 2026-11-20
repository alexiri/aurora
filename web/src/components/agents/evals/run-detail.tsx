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
import {
  useCancelEvalRun,
  useDeleteEvalRun,
  useEvalRun,
  useEvalRuns,
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
function why(r: EvalResult): string {
  const parts: string[] = [];
  if (r.reason_code) parts.push(reasonText(r.reason_code));
  if (r.judge?.rationale) parts.push(`Grader: ${r.judge.rationale}`);
  return parts.join(" ");
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

function Results({ run }: { run: EvalRunDetail }) {
  if (run.results.length === 0) return <EmptyState title="No results yet">Results appear here as each question finishes.</EmptyState>;
  return (
    <>
      <div className="hidden md:block">
        <DataTable caption="Results" className="min-w-0">
          <thead>
            <tr>
              <Th>#</Th>
              <Th>Result</Th>
              <Th>Question</Th>
              <Th>Answer</Th>
              <Th>Why</Th>
              <Th>Refused writes</Th>
            </tr>
          </thead>
          <tbody>
            {run.results.map((r) => (
              <tr key={r.position}>
                <Td className="tabular-nums text-ink-2">{r.position}</Td>
                <Td>
                  <OutcomeBadge outcome={r.outcome} />
                </Td>
                <Td className="max-w-xs whitespace-normal break-words">{excerpt(r.question.text, 120)}</Td>
                <Td className="max-w-sm whitespace-normal break-words text-ink-2">
                  {excerpt(r.answer) || "—"}
                  <ResultDetails r={r} />
                </Td>
                <Td className="max-w-xs whitespace-normal break-words text-ink-2">{why(r) || "—"}</Td>
                <Td className="tabular-nums">{r.refused_writes}</Td>
              </tr>
            ))}
          </tbody>
        </DataTable>
      </div>
      <ItemList aria-label="Results" className="md:hidden">
        {run.results.map((r) => (
          <ListRow key={r.position}>
            <div className="min-w-0 flex-1 space-y-1.5 text-sm">
              <p className="flex flex-wrap items-center gap-2">
                <span className="tabular-nums text-ink-2">{r.position}.</span>
                <OutcomeBadge outcome={r.outcome} />
                {r.refused_writes > 0 ? <span className="text-xs text-muted">{r.refused_writes} refused writes</span> : null}
              </p>
              <p className="break-words text-ink">{excerpt(r.question.text, 120)}</p>
              {r.answer ? <p className="break-words text-xs text-ink-2">{excerpt(r.answer)}</p> : null}
              {why(r) ? <p className="break-words text-xs text-ink-2">{why(r)}</p> : null}
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
}: {
  agentId: string;
  runId: string;
  compareWith?: string;
  canOwn: boolean;
  onBack: () => void;
}) {
  const q = useEvalRun(agentId, runId);
  const cancel = useCancelEvalRun(agentId);
  const del = useDeleteEvalRun(agentId);
  const [confirm, setConfirm] = useState<"cancel" | "delete" | null>(null);
  const back = (
    <Button size="sm" variant="outline" onClick={onBack}>
      All runs
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
  const err = cancel.error ?? del.error;
  return (
    <div className="space-y-6">
      <Section
        id="eval-run"
        title={
          <span className="flex flex-wrap items-center gap-2">
            Run of {fmtDate(run.started_at ?? run.created_at)} <StatusBadge status={run.status} />
          </span>
        }
        description={`${run.trigger === "schedule" ? `Started by the schedule ${run.schedule?.name ?? ""}` : `Started by ${run.started_by?.label ?? "someone who has left"}`}. ${run.finished_at ? `Finished ${fmtDate(run.finished_at)}.` : ""}`}
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
            ["Errors", c.errored],
            ["Not scored", c.not_scored],
            ["Refused writes", c.refused_writes],
          ].map(([k, v]) => (
            <div key={k}>
              <dt className="text-xs text-muted">{k}</dt>
              <dd className="text-base font-semibold tabular-nums text-ink">{v}</dd>
            </div>
          ))}
        </dl>
        {run.judge_usage ? (
          <p className="text-xs text-muted">
            Grader usage: {run.judge_usage.input_tokens} input tokens, {run.judge_usage.output_tokens} output tokens.
          </p>
        ) : null}
        <p className="flex flex-wrap items-center gap-x-2 text-xs text-muted">
          Configuration: <HashValue value={run.fingerprint_hash} label="configuration hash" />
        </p>
      </Section>
      <Section id="eval-results" title="Results" description="Answers and expected answers are visible to the people who can use this agent.">
        <Results run={run} />
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
