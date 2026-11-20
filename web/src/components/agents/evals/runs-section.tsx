"use client";

import { useState } from "react";

import { fmtDate } from "@/components/account/key-table";
import { EmptyState, ErrorState, Loading } from "@/components/states";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { ConfirmDialog } from "@/components/ui/confirm-dialog";
import { DataTable, Td, Th } from "@/components/ui/data-table";
import { LoadMore } from "@/components/ui/load-more";
import { Meter } from "@/components/ui/meter";
import { Notice } from "@/components/ui/notice";
import { ItemList, ListRow, Section } from "@/components/ui/section";
import { useCancelEvalRun, useEvalQuestions, useEvalRuns, useStartEvalRun, type EvalRun } from "@/lib/api/evals";

import { RUN_STATUS, evalErrorMessage, isCode } from "./copy";
import { fmtRate, isActive, progressRatio, startBlockedReason } from "./run-state";

export function RegressionNotice({ run, onCompare }: { run: EvalRun; onCompare?: (fromRunId: string) => void }) {
  if (!run.regression) return null;
  return (
    <Notice tone="pending" role="status" data-testid="regression-notice">
      The pass rate fell by {run.regression.drop_points} points compared with the previous finished run that used the same questions.{" "}
      {onCompare ? (
        <button type="button" className="font-semibold underline" onClick={() => onCompare(run.regression!.from_run_id)}>
          See what changed
        </button>
      ) : null}
    </Notice>
  );
}

function Progress({ run }: { run: EvalRun }) {
  const active = isActive(run);
  const ratio = active ? progressRatio(run) : (run.pass_rate ?? 0) / 100;
  const answered = run.counts.passed + run.counts.failed + run.counts.errored + run.counts.not_scored;
  return (
    <div className="min-w-32 space-y-1">
      <Meter
        ratio={ratio}
        role={active ? "progressbar" : "meter"}
        aria-label={active ? "Run progress" : "Pass rate"}
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={Math.round(ratio * 100)}
      />
      <p className="text-xs text-ink-2">{active ? `${answered} of ${run.counts.total} done` : `${fmtRate(run.pass_rate)} passed`}</p>
    </div>
  );
}

const who = (r: EvalRun) => (r.trigger === "schedule" ? `Schedule${r.schedule ? `: ${r.schedule.name}` : ""}` : (r.started_by?.label ?? "Someone who has left"));

export function StatusBadge({ status }: { status: string }) {
  const s = RUN_STATUS[status] ?? { label: status, tone: "neutral" as const };
  return <Badge tone={s.tone}>{s.label}</Badge>;
}

/** Runs of one agent: start, cancel, and the way into a run's page. */
export function RunsSection({
  agentId,
  frozen,
  onOpen,
}: {
  agentId: string;
  frozen: boolean;
  onOpen: (runId: string, compareWith?: string) => void;
}) {
  const runsQ = useEvalRuns(agentId);
  const questions = useEvalQuestions(agentId);
  const start = useStartEvalRun(agentId);
  const cancel = useCancelEvalRun(agentId);
  const [cancelling, setCancelling] = useState<EvalRun | null>(null);
  if (runsQ.isPending) return <Loading />;
  if (runsQ.isError) return <ErrorState error={runsQ.error} onRetry={() => void runsQ.refetch()} />;
  const runs = runsQ.data.pages.flatMap((p) => p.items);
  const blocked = startBlockedReason(runs, { enabledQuestions: (questions.data ?? []).filter((q) => q.enabled).length, frozen });
  const latest = runs[0];
  // The latest finished run's regression is the one worth a notice on the list; older ones stay on their own page.
  const regressed = latest?.status === "done" ? latest : undefined;

  const doStart = () =>
    start.mutate(undefined, { onSuccess: (r) => onOpen(r.id) });

  return (
    <Section
      id="eval-runs"
      title="Runs"
      description="A run asks every included question in turn and scores the replies. It uses the agent's current instructions, model and skills, and records them."
      actions={
        <Button size="sm" disabled={blocked !== null || start.isPending} aria-describedby={blocked ? "evr-blocked" : undefined} onClick={doStart}>
          Start run
        </Button>
      }
    >
      {blocked ? (
        <p id="evr-blocked" className="text-sm text-ink-2">
          {blocked}
        </p>
      ) : null}
      {start.error ? (
        <Notice tone={isCode(start.error, "eval_run_active", "eval_runs_limited") ? "pending" : "danger"} role="alert">
          {evalErrorMessage(start.error)}
        </Notice>
      ) : null}
      {cancel.error ? (
        <Notice tone="danger" role="alert">
          {evalErrorMessage(cancel.error)}
        </Notice>
      ) : null}
      {regressed ? <RegressionNotice run={regressed} onCompare={(from) => onOpen(regressed.id, from)} /> : null}
      {runs.length === 0 ? (
        <EmptyState title="No runs yet">Start a run to see how the agent answers your questions today. Every later run can be compared with it.</EmptyState>
      ) : (
        <>
          <div className="hidden md:block">
            <DataTable caption="Test runs" className="min-w-0">
              <thead>
                <tr>
                  <Th>Status</Th>
                  <Th>Progress</Th>
                  <Th>Started</Th>
                  <Th>By</Th>
                  <Th>
                    <span className="sr-only">Actions</span>
                  </Th>
                </tr>
              </thead>
              <tbody>
                {runs.map((r) => (
                  <tr key={r.id}>
                    <Td>
                      <StatusBadge status={r.status} />
                    </Td>
                    <Td>
                      <Progress run={r} />
                    </Td>
                    <Td className="whitespace-nowrap text-ink-2">{fmtDate(r.started_at ?? r.created_at)}</Td>
                    <Td className="text-ink-2">{who(r)}</Td>
                    <Td className="text-right">
                      <div className="flex justify-end gap-2">
                        <Button size="sm" variant="outline" onClick={() => onOpen(r.id)}>
                          View
                        </Button>
                        {isActive(r) ? (
                          <Button size="sm" variant="outline" onClick={() => setCancelling(r)}>
                            Cancel run
                          </Button>
                        ) : null}
                      </div>
                    </Td>
                  </tr>
                ))}
              </tbody>
            </DataTable>
          </div>
          <ItemList aria-label="Test runs" className="md:hidden">
            {runs.map((r) => (
              <ListRow
                key={r.id}
                actions={
                  <>
                    <Button size="sm" variant="outline" aria-label={`View run of ${fmtDate(r.started_at ?? r.created_at)}`} onClick={() => onOpen(r.id)}>
                      View
                    </Button>
                    {isActive(r) ? (
                      <Button size="sm" variant="outline" onClick={() => setCancelling(r)}>
                        Cancel run
                      </Button>
                    ) : null}
                  </>
                }
              >
                <div className="min-w-0 flex-1 space-y-1.5 text-sm">
                  <p className="flex flex-wrap items-center gap-2">
                    <StatusBadge status={r.status} />
                    <span className="text-xs text-ink-2">{fmtDate(r.started_at ?? r.created_at)}</span>
                  </p>
                  <Progress run={r} />
                  <p className="text-xs text-muted">{who(r)}</p>
                </div>
              </ListRow>
            ))}
          </ItemList>
          <LoadMore query={runsQ} label="Load more runs" />
        </>
      )}
      <ConfirmDialog
        open={!!cancelling}
        onOpenChange={(o) => {
          if (!o) setCancelling(null);
        }}
        title="Cancel this run?"
        description="The question in progress is stopped and the rest are not scored. Results already recorded are kept."
        confirmLabel="Stop the run"
        danger
        busy={cancel.isPending}
        onConfirm={() => {
          if (cancelling) cancel.mutate(cancelling.id, { onSettled: () => setCancelling(null) });
        }}
      />
    </Section>
  );
}
