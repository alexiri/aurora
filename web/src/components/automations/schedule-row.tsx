"use client";

import Link from "next/link";
import { useState } from "react";

import { AnchorMenuItem, OwnerLabel } from "@/components/automations/owner-controls";
import { ReassignDialog, RowMenu, RowShell } from "@/components/automations/row-parts";
import { ScheduleForm } from "@/components/automations/schedule-form";
import { ErrorState, Loading } from "@/components/states";
import { Altitude } from "@/components/ui/altitude";
import { Button } from "@/components/ui/button";
import { ConfirmDialog } from "@/components/ui/confirm-dialog";
import { Dialog, DialogContent } from "@/components/ui/dialog";
import { LoadMore } from "@/components/ui/load-more";
import { MenuItem, MenuSeparator } from "@/components/ui/menu";
import { ErrorNotice, Notice } from "@/components/ui/notice";
import { ResponsiveTable } from "@/components/ui/responsive-table";
import { TechnicalDetails } from "@/components/ui/technical-details";
import { useUndoToast } from "@/components/ui/undo-toast";
import { When } from "@/components/ui/when";
import { useDeleteSchedule, useRunScheduleNow, useScheduleRuns, useUpdateSchedule, type Schedule, type ScheduleRun } from "@/lib/api/schedules";
import { automationThreadHref } from "@/lib/automation-thread";
import { searchTime } from "@/lib/chat/layout";
import { cronToSpec, whenWords } from "@/lib/cron";
import { label, RUN_RESULT, runReason } from "@/lib/labels";
import { clockIn, runTimeWords } from "@/lib/schedule-words";
import { zoneCity } from "@/lib/cron";

/** (D-407) "Answered", "Skipped by the gate · nothing to report", "Failed · the check could not run". */
export function runOutcomeWords(r: Pick<ScheduleRun, "result" | "reason">): string {
  const reason = runReason(r.result, r.reason);
  return reason ? `${label(RUN_RESULT, r.result)} · ${reason}` : label(RUN_RESULT, r.result);
}

/** The run's thread is named "<Schedule name> · <time>" (the same title the conversation list shows, D-409). */
export function runThreadName(scheduleName: string, firedAt: string, now: Date = new Date()): string {
  const when = searchTime(firedAt, now);
  return when ? `${scheduleName} · ${when}` : scheduleName;
}

/** (schedules-3) "Open details" for a run that has no thread: what the run has, in words, the error under Technical details. */
function RunDetails({ run, onClose }: { run: ScheduleRun; onClose: () => void }) {
  return (
    <Dialog open onOpenChange={(o) => (o ? undefined : onClose())}>
      <DialogContent title="What happened to this run" description="This run did not start a conversation.">
        <div className="space-y-3 text-sm text-ink">
          <p>
            <When value={run.fired_at} /> · {runOutcomeWords(run)}
          </p>
          <TechnicalDetails>
            {[`outcome: ${run.outcome}`, `result: ${run.result}`, run.reason ? `reason: ${run.reason}` : null, run.turn_id ? `turn: ${run.turn_id}` : null, `run: ${run.id}`]
              .filter(Boolean)
              .map((line) => (
                <div key={line}>{line}</div>
              ))}
          </TechnicalDetails>
        </div>
      </DialogContent>
    </Dialog>
  );
}

/** (schedules-3) The runs as a table: When · Outcome · Conversation; stacked rows below 900 px. */
export function Runs({ agentId, id, agentBase, name }: { agentId: string; id: string; agentBase: string; name: string }) {
  const q = useScheduleRuns(agentId, id, true);
  const [details, setDetails] = useState<ScheduleRun | null>(null);
  if (q.isPending) return <Loading />;
  if (q.isError) return <ErrorState error={q.error} onRetry={() => void q.refetch()} />;
  const items = q.data.pages.flatMap((p) => p.items);
  if (items.length === 0) return <p className="text-sm text-muted">No runs yet.</p>;
  return (
    <div className="flex flex-col gap-3">
      <ResponsiveTable<ScheduleRun>
        caption="Runs"
        rows={items}
        rowKey={(r) => r.id}
        title={{ header: "When", cell: (r) => <When value={r.fired_at} /> }}
        columns={[
          { key: "outcome", header: "Outcome", cell: (r) => runOutcomeWords(r) },
          {
            key: "conversation",
            header: "Conversation",
            cell: (r) => {
              const href = automationThreadHref(agentBase, r.conversation_id);
              if (href)
                return (
                  <Link href={href} className="text-accent-ink underline underline-offset-2 hover:no-underline">
                    {runThreadName(name, r.fired_at)}
                  </Link>
                );
              return r.result === "failed" ? (
                <button type="button" className="text-accent-ink underline underline-offset-2 hover:no-underline" onClick={() => setDetails(r)}>
                  Open details
                </button>
              ) : (
                "None"
              );
            },
          },
        ]}
      />
      {details ? <RunDetails run={details} onClose={() => setDetails(null)} /> : null}
      <LoadMore query={q} />
    </div>
  );
}

/** The row's "when": the cadence in words, then the next run or why there is none. */
function whenLine(s: Schedule): string {
  if (!s.cron) {
    // A one-time schedule (P5-SCHED-ONCE): its own words come with Part C.
    if (s.completed_at) return "Done";
    return s.run_at ? `Once, ${runTimeWords(s.run_at, s.timezone)} (${zoneCity(s.timezone)} time)` : "Once";
  }
  const base = whenWords(s.cron, s.timezone);
  if (!s.enabled) return `${base} · paused, won't run until resumed`;
  const next = s.next_runs?.[0] ?? s.next_fire_at;
  return next ? `${base} · next at ${clockIn(next, s.timezone)} (${zoneCity(s.timezone)} time)` : base;
}

export function ScheduleRow({
  agentId,
  s,
  agentBase,
  defaultTimezone,
  workspaceName,
  wsId,
  canOwn,
  initiallyShowRuns = false,
}: {
  agentId: string;
  s: Schedule;
  agentBase: string;
  defaultTimezone?: string;
  workspaceName?: string;
  wsId?: string;
  canOwn?: boolean;
  initiallyShowRuns?: boolean;
}) {
  const [editing, setEditing] = useState(false);
  const [showRuns, setShowRuns] = useState(initiallyShowRuns);
  const [confirm, setConfirm] = useState(false);
  const [reassigning, setReassigning] = useState(false);
  const [started, setStarted] = useState<ScheduleRun | null>(null);
  const run = useRunScheduleNow(agentId);
  const del = useDeleteSchedule(agentId);
  const upd = useUpdateSchedule(agentId);
  const toast = useUndoToast();

  if (editing)
    return (
      <li className="p-4" data-automation-kind="schedule">
        <ScheduleForm agentId={agentId} initial={s} defaultTimezone={defaultTimezone} workspaceName={workspaceName} embedded onDone={() => setEditing(false)} />
      </li>
    );

  const setEnabled = (enabled: boolean, announce: boolean) =>
    upd.mutate(
      { id: s.id, body: { enabled } },
      {
        onSuccess: () => {
          if (announce) toast(`${enabled ? "Resumed" : "Paused"} ${s.name}`, () => setEnabled(!enabled, false));
        },
      },
    );
  const err = run.error ?? del.error ?? upd.error;
  const threadHref = started ? automationThreadHref(agentBase, started.conversation_id) : null;
  const custom = s.cron ? cronToSpec(s.cron).preset === "custom" : false;
  const last = s.last_fired_at ? (
    <>
      Last run: <When value={s.last_fired_at} />
      {s.last_result ? ` · ${label(RUN_RESULT, s.last_result)}` : ""}
    </>
  ) : (
    "Last run: never"
  );

  return (
    <RowShell
      kind="schedule"
      name={s.name}
      on={s.enabled}
      when={whenLine(s)}
      body={s.prompt}
      meta={
        <>
          {s.paused_by ? `Paused by ${s.paused_by.display_name} · ` : ""}
          {last} · <OwnerLabel anchor={s.anchor} creator={s.created_by} />
          {s.origin_conversation_id ? " · Also posts in the chat it was made in" : ""}
        </>
      }
      notices={
        <>
          {s.disabled_reason === "authorization" ? (
            <Notice tone="pending" role="status">
              Paused because its owner lost access. Reassign it to a current member to bring it back.
            </Notice>
          ) : null}
          {s.disabled_reason === "failures" && !s.enabled ? (
            <Notice tone="pending" role="status">
              Paused after too many failed runs. Resume it when the cause is fixed.
            </Notice>
          ) : null}
          {started ? (
            <p role="status" className="text-sm text-ink-2" data-testid="run-started">
              {started.result === "skipped_by_gate" ? "Skipped by the gate · nothing to report" : "Started"}
              {threadHref ? (
                <>
                  {" · "}
                  <Link href={threadHref} className="font-medium text-accent-ink underline underline-offset-2 hover:no-underline">
                    Open the conversation
                  </Link>
                </>
              ) : null}
            </p>
          ) : null}
          <ErrorNotice error={err} />
        </>
      }
      actions={
        <>
          <Button
            size="sm"
            disabled={run.isPending || !s.enabled}
            onClick={() =>
              run.mutate(s.id, {
                onSuccess: (r) => {
                  setStarted(r);
                  // (schedules-3) The same words as a toast at every width, linking the run's own thread.
                  const href = automationThreadHref(agentBase, r.conversation_id);
                  if (r.result === "skipped_by_gate") toast("Skipped by the gate · nothing to report");
                  else toast("Started", undefined, { success: true, link: href ? { label: "Open the conversation", href } : undefined });
                },
              })
            }>
            Run now
          </Button>
          <Button size="sm" variant="outline" onClick={() => setEditing(true)}>
            Edit
          </Button>
          {/* Below 640 px Runs stays on the row; only Reassign and Delete go into the menu. */}
          <Button size="sm" variant="outline" className="sm:hidden" aria-expanded={showRuns} onClick={() => setShowRuns((v) => !v)}>
            Runs
          </Button>
          <RowMenu name={s.name}>
            <MenuItem className="max-sm:hidden" onSelect={() => setShowRuns((v) => !v)}>
              Runs
            </MenuItem>
            <MenuItem disabled={upd.isPending} onSelect={() => setEnabled(!s.enabled, true)}>
              {s.enabled ? "Pause" : "Resume"}
            </MenuItem>
            {canOwn && wsId ? <MenuItem onSelect={() => setReassigning(true)}>Reassign…</MenuItem> : null}
            {canOwn ? (
              <AnchorMenuItem name={s.name} anchor={s.anchor} busy={upd.isPending} onChange={(anchor) => upd.mutate({ id: s.id, body: { anchor } })} />
            ) : null}
            <MenuSeparator />
            <MenuItem className="text-danger-ink" onSelect={() => setConfirm(true)}>
              Delete…
            </MenuItem>
          </RowMenu>
        </>
      }
    >
      {custom ? (
        <Altitude id="tech.details" level="setup">
          <TechnicalDetails>{s.cron}</TechnicalDetails>
        </Altitude>
      ) : null}
      {reassigning && wsId ? (
        <ReassignDialog open onOpenChange={setReassigning} agentId={agentId} wsId={wsId} kind="schedule" id={s.id} name={s.name} creatorId={s.created_by?.id} disabledReason={s.disabled_reason} />
      ) : null}
      <ConfirmDialog
        open={confirm}
        onOpenChange={setConfirm}
        title={`Delete ${s.name}?`}
        description="It stops running. Its past conversations are kept."
        confirmLabel="Delete schedule"
        danger
        busy={del.isPending}
        onConfirm={() => del.mutate(s.id, { onSuccess: () => setConfirm(false) })}
      />
      {showRuns ? <Runs agentId={agentId} id={s.id} agentBase={agentBase} name={s.name} /> : null}
    </RowShell>
  );
}
