"use client";

import { useState, type FormEvent } from "react";

import { fmtDate } from "@/components/account/key-table";
import { EmptyState, ErrorState, Loading } from "@/components/states";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { ConfirmDialog, FormDialog } from "@/components/ui/confirm-dialog";
import { DataTable, Td, Th } from "@/components/ui/data-table";
import { Field, inputClass } from "@/components/ui/field";
import { CheckboxField } from "@/components/ui/form";
import { Notice } from "@/components/ui/notice";
import { ItemList, ListRow, Section } from "@/components/ui/section";
import { fieldErrors } from "@/lib/api/errors";
import {
  EVAL_SCHEDULES_PER_AGENT,
  useCreateEvalSchedule,
  useDeleteEvalSchedule,
  useEvalSchedules,
  usePatchEvalSchedule,
  useStartEvalRun,
  type EvalSchedule,
} from "@/lib/api/evals";
import { cronFieldCount } from "@/lib/api/schedules";

import { evalErrorMessage } from "./copy";
import { describeCron, firesMoreThanHourly } from "./cron-words";

const OUTCOME_WORDS: Record<string, string> = {
  ok: "Started a run",
  failed: "Could not start a run",
  skipped_busy: "Skipped: a run was in progress or the day's limit was reached",
  skipped_frozen: "Skipped: the agent was frozen",
};

const DISABLED_WORDS: Record<string, string> = {
  user: "You turned it off.",
  authorization: "Its owner lost access to the agent.",
  failures: "Five runs in a row could not start.",
};

function ScheduleDialog({
  agentId,
  schedule,
  defaultTimezone,
  canOwn,
  onClose,
}: {
  agentId: string;
  schedule?: EvalSchedule;
  defaultTimezone?: string;
  canOwn: boolean;
  onClose: () => void;
}) {
  const create = useCreateEvalSchedule(agentId);
  const patch = usePatchEvalSchedule(agentId);
  const [name, setName] = useState(schedule?.name ?? "");
  const [cron, setCron] = useState(schedule?.cron ?? "0 6 * * *");
  const [timezone, setTimezone] = useState(schedule?.timezone ?? defaultTimezone ?? "UTC");
  const [enabled, setEnabled] = useState(schedule?.enabled ?? true);
  const [anchored, setAnchored] = useState(false);
  const [local, setLocal] = useState<string | null>(null);
  const mut = schedule ? patch : create;
  const fe = fieldErrors(mut.error);
  const words = describeCron(cron, timezone);
  const tooOften = firesMoreThanHourly(cron);

  function submit(e: FormEvent) {
    e.preventDefault();
    setLocal(null);
    if (cronFieldCount(cron) !== 5) return setLocal("A cron expression has exactly 5 fields: minute, hour, day of month, month, day of week.");
    const body = { name: name.trim(), cron: cron.trim(), timezone: timezone.trim() };
    if (schedule) patch.mutate({ id: schedule.id, body: { ...body, enabled } }, { onSuccess: onClose });
    else create.mutate({ ...body, ...(anchored ? { anchor: "workspace" } : {}) }, { onSuccess: onClose });
  }

  return (
    <FormDialog
      open
      onOpenChange={(o) => {
        if (!o) onClose();
      }}
      title={schedule ? "Edit eval schedule" : "New eval schedule"}
      description="Starts a run of all included questions at set times. Runs of a schedule count toward the daily limit."
      submitLabel={schedule ? "Save schedule" : "Create schedule"}
      busy={mut.isPending}
      onSubmit={submit}
    >
      <Field id="evs-name" label="Name" error={fe.name}>
        <input id="evs-name" className={inputClass} value={name} required onChange={(e) => setName(e.target.value)} />
      </Field>
      <Field id="evs-cron" label="When (cron, 5 fields)" error={fe.cron ?? local ?? undefined} hint="For example 0 6 * * * is every day at 06:00. At least an hour apart.">
        <input id="evs-cron" className={`${inputClass} font-mono`} value={cron} required onChange={(e) => setCron(e.target.value)} />
      </Field>
      <p className="text-sm text-ink-2" data-testid="cron-preview" aria-live="polite">
        {tooOften ? "This fires more than once an hour. Eval schedules must be at least an hour apart." : (words ?? "The exact times are shown after saving.")}
      </p>
      <Field id="evs-tz" label="Timezone" error={fe.timezone} hint="An IANA name, for example Europe/Zurich.">
        <input id="evs-tz" className={inputClass} value={timezone} onChange={(e) => setTimezone(e.target.value)} />
      </Field>
      {schedule ? <CheckboxField id="evs-enabled" label="On" checked={enabled} onChange={setEnabled} /> : null}
      {!schedule && canOwn ? (
        <CheckboxField id="evs-anchor" label="Keep it running if I leave" hint="Anchors the schedule to the workspace instead of to you." checked={anchored} onChange={setAnchored} />
      ) : null}
      {mut.error && Object.keys(fe).length === 0 ? (
        <Notice tone="danger" role="alert">
          {evalErrorMessage(mut.error)}
        </Notice>
      ) : null}
    </FormDialog>
  );
}

export function SchedulesSection({ agentId, defaultTimezone, canOwn, onRunStarted }: { agentId: string; defaultTimezone?: string; canOwn: boolean; onRunStarted: (id: string) => void }) {
  const q = useEvalSchedules(agentId);
  const patch = usePatchEvalSchedule(agentId);
  const del = useDeleteEvalSchedule(agentId);
  const start = useStartEvalRun(agentId);
  const [dialog, setDialog] = useState<{ schedule?: EvalSchedule } | null>(null);
  const [removing, setRemoving] = useState<EvalSchedule | null>(null);
  if (q.isPending) return <Loading />;
  if (q.isError) return <ErrorState error={q.error} onRetry={() => void q.refetch()} />;
  const items = q.data;
  const full = items.length >= EVAL_SCHEDULES_PER_AGENT;
  const err = patch.error ?? del.error ?? start.error;

  const state = (s: EvalSchedule) =>
    s.enabled ? (
      <Badge tone="accent">On</Badge>
    ) : (
      <span className="flex flex-col items-start gap-1">
        <Badge>Off</Badge>
        <span className="max-w-48 whitespace-normal text-xs text-ink-2">{DISABLED_WORDS[s.disabled_reason ?? "user"] ?? ""}</span>
      </span>
    );
  const actions = (s: EvalSchedule) => (
    <>
      <Button size="sm" variant="outline" disabled={start.isPending} onClick={() => start.mutate(undefined, { onSuccess: (r) => onRunStarted(r.id) })}>
        Run now
      </Button>
      <Button size="sm" variant="outline" aria-label={`Edit ${s.name}`} onClick={() => setDialog({ schedule: s })}>
        Edit
      </Button>
      <Button size="sm" variant="outline" disabled={patch.isPending} aria-label={`${s.enabled ? "Turn off" : "Turn on"} ${s.name}`} onClick={() => patch.mutate({ id: s.id, body: { enabled: !s.enabled } })}>
        {s.enabled ? "Turn off" : "Turn on"}
      </Button>
      <Button size="sm" variant="outline" aria-label={`Delete ${s.name}`} onClick={() => setRemoving(s)}>
        Delete
      </Button>
    </>
  );

  return (
    <Section
      id="eval-schedules"
      title="Schedules"
      description={`Run the questions on a timetable, at least an hour apart. Up to ${EVAL_SCHEDULES_PER_AGENT} schedules; ${items.length} in use.`}
      actions={
        <Button size="sm" disabled={full} onClick={() => setDialog({})}>
          New schedule
        </Button>
      }
    >
      {full ? (
        <Notice tone="pending" role="status">
          This agent has {EVAL_SCHEDULES_PER_AGENT} eval schedules, the most it can have. Delete one to add another.
        </Notice>
      ) : null}
      {err ? (
        <Notice tone="danger" role="alert">
          {evalErrorMessage(err)}
        </Notice>
      ) : null}
      {items.length === 0 ? (
        <EmptyState title="No eval schedules">A schedule starts a run for you, for example every morning, so a change that makes the agent worse shows up even when nobody is looking.</EmptyState>
      ) : (
        <>
          <div className="hidden md:block">
            <DataTable caption="Eval schedules" className="min-w-0">
              <thead>
                <tr>
                  <Th>Name</Th>
                  <Th>When</Th>
                  <Th>Next run</Th>
                  <Th>Last</Th>
                  <Th>State</Th>
                  <Th>
                    <span className="sr-only">Actions</span>
                  </Th>
                </tr>
              </thead>
              <tbody>
                {items.map((s) => (
                  <tr key={s.id}>
                    <Td className="whitespace-normal break-words font-medium">{s.name}</Td>
                    <Td className="whitespace-normal">
                      <code className="font-mono text-xs">{s.cron}</code>
                      <span className="block text-xs text-muted">{describeCron(s.cron, s.timezone) ?? s.timezone}</span>
                    </Td>
                    <Td className="whitespace-nowrap text-ink-2">{s.next_fire_at ? fmtDate(s.next_fire_at) : "—"}</Td>
                    <Td className="max-w-xs whitespace-normal text-ink-2">{s.last_outcome ? (OUTCOME_WORDS[s.last_outcome] ?? s.last_outcome) : "Never"}</Td>
                    <Td>{state(s)}</Td>
                    <Td className="text-right">
                      <div className="flex justify-end gap-2">{actions(s)}</div>
                    </Td>
                  </tr>
                ))}
              </tbody>
            </DataTable>
          </div>
          <ItemList aria-label="Eval schedules" className="md:hidden">
            {items.map((s) => (
              <ListRow key={s.id} actions={actions(s)}>
                <div className="min-w-0 flex-1 space-y-1 text-sm">
                  <p className="flex flex-wrap items-center gap-2">
                    <span className="break-words font-medium text-ink">{s.name}</span>
                    {state(s)}
                  </p>
                  <p className="text-xs text-ink-2">
                    <code className="font-mono">{s.cron}</code> · {describeCron(s.cron, s.timezone) ?? s.timezone}
                  </p>
                  <p className="text-xs text-muted">
                    Next: {s.next_fire_at ? fmtDate(s.next_fire_at) : "—"} · Last: {s.last_outcome ? (OUTCOME_WORDS[s.last_outcome] ?? s.last_outcome) : "never"}
                  </p>
                </div>
              </ListRow>
            ))}
          </ItemList>
        </>
      )}
      {dialog ? <ScheduleDialog key={dialog.schedule?.id ?? "new"} agentId={agentId} schedule={dialog.schedule} defaultTimezone={defaultTimezone} canOwn={canOwn} onClose={() => setDialog(null)} /> : null}
      <ConfirmDialog
        open={!!removing}
        onOpenChange={(o) => {
          if (!o) setRemoving(null);
        }}
        title="Delete this schedule?"
        description={removing ? `${removing.name} stops starting runs. Runs it already started are kept.` : undefined}
        confirmLabel="Delete schedule"
        danger
        busy={del.isPending}
        onConfirm={() => {
          if (removing) del.mutate(removing.id, { onSettled: () => setRemoving(null) });
        }}
      />
    </Section>
  );
}
