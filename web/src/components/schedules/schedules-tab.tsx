"use client";

import { useState, type FormEvent } from "react";

import { fmtDate } from "@/components/account/key-table";
import { EmptyState, ErrorState, Loading } from "@/components/states";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { DataTable, Td, Th } from "@/components/ui/data-table";
import { Field, inputClass } from "@/components/ui/field";
import { CheckboxField, FormActions } from "@/components/ui/form";
import { Notice } from "@/components/ui/notice";
import { ItemList, Section } from "@/components/ui/section";
import { AnchorSwitch, OwnerLabel, ReassignPanel } from "@/components/automations/owner-controls";
import { errorMessage, fieldErrors } from "@/lib/api/errors";
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

function Runs({ agentId, id }: { agentId: string; id: string }) {
  const q = useScheduleRuns(agentId, id, true);
  if (q.isPending) return <Loading />;
  if (q.isError) return <ErrorState error={q.error} onRetry={() => void q.refetch()} />;
  if (q.data.items.length === 0) return <p className="text-sm text-muted">No runs yet.</p>;
  return (
    <DataTable caption="Runs">
      <thead>
        <tr>
          <Th>Fired</Th>
          <Th>Outcome</Th>
        </tr>
      </thead>
      <tbody>
        {q.data.items.map((r) => (
          <tr key={r.id}>
            <Td className="whitespace-nowrap">{fmtDate(r.fired_at)}</Td>
            <Td>{r.outcome}</Td>
          </tr>
        ))}
      </tbody>
    </DataTable>
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
      <Field id="sch-gate" label="Gate command (optional)" error={fe.gate_command} hint="Runs before each fire; the agent is only woken when it reports something to do.">
        <input id="sch-gate" className={`${inputClass} font-mono`} value={gateCommand} onChange={(e) => setGateCommand(e.target.value)} />
      </Field>
      <Field id="sch-gate-desc" label="Gate description (optional)" error={fe.gate_description}>
        <input id="sch-gate-desc" className={inputClass} value={gateDescription} onChange={(e) => setGateDescription(e.target.value)} />
      </Field>
      {!initial && canOwn ? (
        <CheckboxField id="sch-anchored" label="Anchor to workspace (outlives any single member)" checked={anchored} onChange={setAnchored} />
      ) : null}
      {initial ? <CheckboxField id="sch-enabled" label="Enabled" checked={enabled} onChange={setEnabled} /> : null}
      {mut.error && Object.keys(fe).length === 0 ? (
        <Notice tone="danger" role="alert">
          {errorMessage(mut.error)}
        </Notice>
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
          {s.enabled ? "Enabled" : `Disabled${s.disabled_reason ? ` (${s.disabled_reason})` : ""}`}
        </Badge>
      </div>
      <p className="whitespace-pre-wrap text-sm text-ink-2">{s.prompt}</p>
      <p className="text-xs text-muted">
        Next: {s.next_fire_at ? fmtDate(s.next_fire_at) : "—"} · Last: {s.last_fired_at ? fmtDate(s.last_fired_at) : "never"} ·{" "}
        <OwnerLabel anchor={s.anchor} creator={s.created_by} />
      </p>
      {s.disabled_reason === "authorization" ? (
        <Notice tone="pending" role="status">
          Disabled because its owner lost access. Reassign it to a current member to bring it back.
        </Notice>
      ) : null}
      {canOwn ? <AnchorSwitch name={s.name} anchor={s.anchor} busy={upd.isPending} onChange={(anchor) => upd.mutate({ id: s.id, body: { anchor } })} /> : null}
      {reassigning && wsId ? (
        <ReassignPanel agentId={agentId} wsId={wsId} kind="schedule" id={s.id} name={s.name} creatorId={s.created_by?.id} disabledReason={s.disabled_reason} onClose={() => setReassigning(false)} />
      ) : null}
      {err ? (
        <Notice tone="danger" role="alert">
          {errorMessage(err)}
        </Notice>
      ) : null}
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
          {s.enabled ? "Disable" : "Enable"}
        </Button>
        <Button size="sm" variant="outline" aria-expanded={showRuns} onClick={() => setShowRuns((v) => !v)}>
          {showRuns ? "Hide runs" : "Runs"}
        </Button>
        {confirm ? (
          <Button size="sm" variant="danger" disabled={del.isPending} onClick={() => del.mutate(s.id)}>
            Confirm delete {s.name}
          </Button>
        ) : (
          <Button size="sm" variant="outline" onClick={() => setConfirm(true)}>
            Delete
          </Button>
        )}
      </div>
      {showRuns ? <Runs agentId={agentId} id={s.id} /> : null}
    </li>
  );
}

export function SchedulesTab({ agentId, defaultTimezone, wsId, canOwn }: { agentId: string; defaultTimezone?: string; wsId?: string; canOwn?: boolean }) {
  const q = useSchedules(agentId);
  const [creating, setCreating] = useState(false);
  if (q.isPending) return <Loading />;
  if (q.isError) return <ErrorState error={q.error} onRetry={() => void q.refetch()} />;
  return (
    <Section
      id="schedules"
      title="Schedules"
      description="Wake the agent with a prompt on a cron."
      actions={
        !creating ? (
          <Button size="sm" onClick={() => setCreating(true)}>
            New schedule
          </Button>
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
          ask="Ask a workspace owner to set one up."
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
