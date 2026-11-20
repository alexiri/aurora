"use client";

import { useState, type FormEvent } from "react";

import { fmtDate } from "@/components/account/key-table";
import { EmptyState, ErrorState, Loading } from "@/components/states";
import { Altitude } from "@/components/ui/altitude";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { DataTable, Td, Th } from "@/components/ui/data-table";
import { Field, inputClass } from "@/components/ui/field";
import { CheckboxField, FormActions } from "@/components/ui/form";
import { LoadMore } from "@/components/ui/load-more";
import { ConfirmDialog } from "@/components/ui/confirm-dialog";
import { ErrorNotice, Notice } from "@/components/ui/notice";
import { ItemList, Section } from "@/components/ui/section";
import { AnchorSwitch, OwnerLabel, ReassignPanel } from "@/components/automations/owner-controls";
import { fieldErrors } from "@/lib/api/errors";
import { MainAction } from "@/components/ui/main-action";
import { useWhoCan } from "@/lib/api/who-can";
import { useVerb } from "@/lib/palette-verb";
import {
  cronFieldCount,
  useCreateSchedule,
  useDeleteSchedule,
  useRunScheduleNow,
  useScheduleRuns,
  useSchedules,
  useUpdateSchedule,
  type Schedule,
} from "@/lib/api/schedules";

/** (D-407) A run's outcome in words. The log only stores how the run started; "Running" comes with the turn's state (brief 14). */
const RUN_OUTCOME: Record<string, string> = {
  fired: "Answered",
  gated_quiet: "Skipped by the gate",
  gate_failed: "Failed",
  skipped_unauthorized: "Skipped: its owner can no longer run it",
  coalesced: "Skipped: it was already running",
};

function Runs({ agentId, id }: { agentId: string; id: string }) {
  const q = useScheduleRuns(agentId, id, true);
  if (q.isPending) return <Loading />;
  if (q.isError) return <ErrorState error={q.error} onRetry={() => void q.refetch()} />;
  const items = q.data.pages.flatMap((p) => p.items);
  if (items.length === 0) return <p className="text-sm text-muted">No runs yet.</p>;
  return (
    <div className="flex flex-col gap-3">
      <DataTable caption="Runs">
        <thead>
          <tr>
            <Th>When</Th>
            <Th>Outcome</Th>
          </tr>
        </thead>
        <tbody>
          {items.map((r) => (
            <tr key={r.id}>
              <Td className="whitespace-nowrap">{fmtDate(r.fired_at)}</Td>
              <Td>{RUN_OUTCOME[r.outcome] ?? r.outcome}</Td>
            </tr>
          ))}
        </tbody>
      </DataTable>
      <LoadMore query={q} />
    </div>
  );
}

export function ScheduleForm({
  agentId,
  initial,
  defaultTimezone,
  canOwn,
  onDone,
}: {
  agentId: string;
  initial?: Schedule;
  defaultTimezone?: string;
  /** Workspace owners may anchor a new schedule to the workspace. */
  canOwn?: boolean;
  onDone: () => void;
}) {
  const create = useCreateSchedule(agentId);
  const update = useUpdateSchedule(agentId);
  const [name, setName] = useState(initial?.name ?? "");
  const [prompt, setPrompt] = useState(initial?.prompt ?? "");
  const [cron, setCron] = useState(initial?.cron ?? "");
  const [timezone, setTimezone] = useState(initial?.timezone ?? defaultTimezone ?? "UTC");
  const [gateCommand, setGateCommand] = useState(initial?.gate_command ?? "");
  const [gateDescription, setGateDescription] = useState(initial?.gate_description ?? "");
  const [enabled, setEnabled] = useState(initial?.enabled ?? true);
  const [anchored, setAnchored] = useState(false);
  const [localErr, setLocalErr] = useState<string | null>(null);
  const mut = initial ? update : create;
  const fe = fieldErrors(mut.error);
  const pending = mut.isPending;

  function submit(e: FormEvent) {
    e.preventDefault();
    setLocalErr(null);
    if (cronFieldCount(cron) !== 5) {
      setLocalErr("Cron must have exactly 5 fields (minute hour day month weekday).");
      return;
    }
    const gate = { gate_command: gateCommand.trim() || null, gate_description: gateDescription.trim() || null };
    if (initial) {
      update.mutate(
        { id: initial.id, body: { name, prompt, cron: cron.trim(), timezone, enabled, ...gate } },
        { onSuccess: onDone },
      );
    } else {
      create.mutate(
        { name, prompt, cron: cron.trim(), timezone, ...gate, ...(anchored ? { anchor: "workspace" } : {}) },
        { onSuccess: onDone },
      );
    }
  }

  return (
    <form onSubmit={submit} aria-label={initial ? "Edit schedule" : "New schedule"} className="space-y-4 rounded-card border border-line bg-surface p-4 shadow-card sm:p-5">
      <Field id="sch-name" label="Name" error={fe.name}>
        <input id="sch-name" className={inputClass} value={name} required onChange={(e) => setName(e.target.value)} />
      </Field>
      <Field id="sch-prompt" label="Prompt" error={fe.prompt}>
        <textarea id="sch-prompt" className={inputClass} rows={3} value={prompt} required onChange={(e) => setPrompt(e.target.value)} />
      </Field>
      <div className="grid gap-4 sm:grid-cols-2">
        <Field id="sch-cron" label="Cron (5 fields)" error={fe.cron ?? localErr ?? undefined} hint="e.g. 0 9 * * 1-5">
          <input id="sch-cron" className={`${inputClass} font-mono`} value={cron} required onChange={(e) => setCron(e.target.value)} />
        </Field>
        <Field id="sch-tz" label="Timezone" error={fe.timezone} hint="IANA name, e.g. Europe/Zurich">
          <input id="sch-tz" className={inputClass} value={timezone} onChange={(e) => setTimezone(e.target.value)} />
        </Field>
      </div>
      {/* (P5-REFINE-3) Expert; shown anyway when this schedule already has a gate or the server rejected one. */}
      <Altitude id="auto.gate" level="expert" promote={!!initial?.gate_command || !!fe.gate_command || !!fe.gate_description}>
        <Field id="sch-gate" label="Only run when… (a check command, optional)" error={fe.gate_command} hint="Runs before each run; the agent is only woken when it reports something to do.">
          <input id="sch-gate" className={`${inputClass} font-mono`} value={gateCommand} onChange={(e) => setGateCommand(e.target.value)} />
        </Field>
        <Field id="sch-gate-desc" label="What the check looks for (optional)" error={fe.gate_description}>
          <input id="sch-gate-desc" className={inputClass} value={gateDescription} onChange={(e) => setGateDescription(e.target.value)} />
        </Field>
      </Altitude>
      {!initial && canOwn ? (
        <Altitude id="auto.anchor" level="expert">
          <CheckboxField id="sch-anchored" label="Keep running if I leave (outlives any single member)" checked={anchored} onChange={setAnchored} />
        </Altitude>
      ) : null}
      {initial ? <CheckboxField id="sch-enabled" label="On" checked={enabled} onChange={setEnabled} /> : null}
      {mut.error && Object.keys(fe).length === 0 ? (
        <ErrorNotice error={mut.error} />
      ) : null}
      <FormActions className="gap-2">
        <Button type="submit" disabled={pending}>
          {initial ? "Save" : "Create schedule"}
        </Button>
        <Button type="button" variant="outline" onClick={onDone}>
          Cancel
        </Button>
      </FormActions>
    </form>
  );
}

function ScheduleRow({ agentId, s, defaultTimezone, wsId, canOwn }: { agentId: string; s: Schedule; defaultTimezone?: string; wsId?: string; canOwn?: boolean }) {
  const [editing, setEditing] = useState(false);
  const [showRuns, setShowRuns] = useState(false);
  const [confirm, setConfirm] = useState(false);
  const [reassigning, setReassigning] = useState(false);
  const run = useRunScheduleNow(agentId);
  const del = useDeleteSchedule(agentId);
  const upd = useUpdateSchedule(agentId);
  if (editing)
    return (
      <li className="bg-canvas p-3 sm:p-4">
        <ScheduleForm agentId={agentId} initial={s} defaultTimezone={defaultTimezone} onDone={() => setEditing(false)} />
      </li>
    );
  const err = run.error ?? del.error ?? upd.error;
  return (
    <li className="space-y-3 p-4">
      <div className="flex flex-wrap items-center gap-x-3 gap-y-1">
        <h3 className="font-display text-base font-semibold text-ink">{s.name}</h3>
        <code className="font-mono text-xs text-ink-2">{s.cron}</code>
        <span className="text-xs text-muted">{s.timezone}</span>
        <Badge className="sm:ml-auto" tone={s.enabled ? "accent" : "neutral"}>
          {s.enabled ? "On" : "Paused"}
        </Badge>
      </div>
      <p className="whitespace-pre-wrap text-sm text-ink-2">{s.prompt}</p>
      <p className="text-xs text-muted">
        Next: {s.next_fire_at ? fmtDate(s.next_fire_at) : "—"} · Last: {s.last_fired_at ? fmtDate(s.last_fired_at) : "never"} ·{" "}
        <OwnerLabel anchor={s.anchor} creator={s.created_by} />
      </p>
      {s.disabled_reason === "authorization" ? (
        <Notice tone="pending" role="status">
          Paused because its owner lost access. Reassign it to a current member to bring it back.
        </Notice>
      ) : null}
      {canOwn ? <AnchorSwitch name={s.name} anchor={s.anchor} busy={upd.isPending} onChange={(anchor) => upd.mutate({ id: s.id, body: { anchor } })} /> : null}
      {reassigning && wsId ? (
        <ReassignPanel agentId={agentId} wsId={wsId} kind="schedule" id={s.id} name={s.name} creatorId={s.created_by?.id} disabledReason={s.disabled_reason} onClose={() => setReassigning(false)} />
      ) : null}
      <ErrorNotice error={err} />
      <div className="flex flex-wrap gap-2">
        <Button size="sm" variant="outline" disabled={run.isPending} onClick={() => run.mutate(s.id)}>
          Run now
        </Button>
        <Button size="sm" variant="outline" onClick={() => setEditing(true)}>
          Edit
        </Button>
        {canOwn && wsId ? (
          <Button size="sm" variant="outline" aria-expanded={reassigning} onClick={() => setReassigning((v) => !v)}>
            Reassign…
          </Button>
        ) : null}
        <Button size="sm" variant="outline" disabled={upd.isPending} onClick={() => upd.mutate({ id: s.id, body: { enabled: !s.enabled } })}>
          {s.enabled ? "Turn off" : "Turn on"}
        </Button>
        <Button size="sm" variant="outline" aria-expanded={showRuns} onClick={() => setShowRuns((v) => !v)}>
          {showRuns ? "Hide runs" : "Runs"}
        </Button>
        <Button size="sm" variant="outline" onClick={() => setConfirm(true)}>
          Delete
        </Button>
      </div>
      <ConfirmDialog
        open={confirm}
        onOpenChange={setConfirm}
        title={`Delete ${s.name}?`}
        description="It stops starting runs. Conversations it already started are kept."
        confirmLabel="Delete schedule"
        danger
        busy={del.isPending}
        onConfirm={() => del.mutate(s.id, { onSuccess: () => setConfirm(false) })}
      />
      {showRuns ? <Runs agentId={agentId} id={s.id} /> : null}
    </li>
  );
}

export function SchedulesTab({ agentId, defaultTimezone, wsId, canOwn }: { agentId: string; defaultTimezone?: string; wsId?: string; canOwn?: boolean }) {
  const q = useSchedules(agentId);
  const [creating, setCreating] = useState(false);
  // (P5-REFINE-19) The palette's "New schedule for <Agent>" opens the form the button opens.
  useVerb("new-schedule", () => setCreating(true), canOwn === true);
  // (C-916) Who sets up a schedule, for a viewer who cannot.
  const who = useWhoCan({ workspaceId: wsId }, "workspace_owner", "add_schedule");
  if (q.isPending) return <Loading />;
  if (q.isError) return <ErrorState error={q.error} onRetry={() => void q.refetch()} />;
  return (
    <Section
      id="schedules"
      title="Schedules"
      description="Wake the agent with a prompt on a cron."
      actions={
        !creating ? (
          <MainAction canAct={Boolean(canOwn)} whoCan={{ ...who, action: "add_schedule" }}>
            <Button size="sm" onClick={() => setCreating(true)}>
              New schedule
            </Button>
          </MainAction>
        ) : null
      }
    >
      {creating ? <ScheduleForm agentId={agentId} defaultTimezone={defaultTimezone} canOwn={canOwn} onDone={() => setCreating(false)} /> : null}
      {q.data.items.length === 0 && !creating ? (
        <EmptyState
          title="No schedules"
          when="Use one for recurring work, such as a summary every morning."
          action={
            canOwn ? (
              <Button size="sm" onClick={() => setCreating(true)}>
                New schedule
              </Button>
            ) : undefined
          }
          ask={{ whoCan: who, canAct: Boolean(canOwn) }}
        >
          A schedule wakes the agent with a prompt at set times, such as every day at 9.
        </EmptyState>
      ) : null}
      {q.data.items.length > 0 ? (
        <ItemList aria-label="Schedule list">
          {q.data.items.map((s) => (
            <ScheduleRow key={s.id} agentId={agentId} s={s} defaultTimezone={defaultTimezone} wsId={wsId} canOwn={canOwn} />
          ))}
        </ItemList>
      ) : null}
    </Section>
  );
}
