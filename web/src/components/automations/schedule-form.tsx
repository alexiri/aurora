"use client";

import { useEffect, useMemo, useState, type FormEvent } from "react";

import { Altitude } from "@/components/ui/altitude";
import { Button } from "@/components/ui/button";
import { Field, inputClass } from "@/components/ui/field";
import { CheckboxField, FormActions } from "@/components/ui/form";
import { ErrorNotice } from "@/components/ui/notice";
import { Segmented } from "@/components/ui/segmented";
import { Combobox } from "@/components/ui/combobox";
import { Select } from "@/components/ui/select";
import { useAltitude } from "@/lib/altitude";
import { fieldErrors } from "@/lib/api/errors";
import { cronFieldCount, useCreateSchedule, useSchedulePreview, useUpdateSchedule, type Schedule } from "@/lib/api/schedules";
import { useMoreControls } from "@/lib/api/use-more-controls";
import {
  cadenceWords,
  clock,
  cronToSpec,
  DEFAULT_SPEC,
  isTooFrequent,
  MIN_GAP_MINUTES,
  MONTH_NAMES,
  PRESETS,
  runAtToSpec,
  specToCron,
  specToRunAt,
  WEEKDAY_NAMES,
  whenWords,
  zoneCity,
  type SchedulePreset,
  type ScheduleSpec,
} from "@/lib/cron";
import { browserTimeZone } from "@/lib/timezone";
import { nextWords, onceLocalWords, tomorrowIn } from "@/lib/schedule-words";

/** The wait before a changed cadence is previewed by the API (rate class `check`, 10 a minute); the answer is cached 60 s. */
export const PREVIEW_DEBOUNCE_MS = 1000;

function useDebounced<T>(value: T, ms: number): T {
  const [v, setV] = useState(value);
  useEffect(() => {
    const t = setTimeout(() => setV(value), ms);
    return () => clearTimeout(t);
  }, [value, ms]);
  return v;
}

/** Every IANA zone the browser knows (a short list where `Intl.supportedValuesOf` is missing). */
function allZones(): string[] {
  try {
    const f = (Intl as unknown as { supportedValuesOf?: (k: string) => string[] }).supportedValuesOf;
    const zones = f ? f("timeZone") : [];
    if (zones.length) return zones.includes("UTC") ? zones : ["UTC", ...zones];
  } catch {
    // fall through
  }
  return ["UTC", "Europe/Zurich", "Europe/London", "America/New_York", "America/Los_Angeles", "Asia/Tokyo"];
}

/** The time zone choices: the workspace's first (its name in the detail), the browser's, then all. */
export function zoneOptions(workspaceZone: string | undefined, workspaceName: string | undefined, current: string): { value: string; label: string }[] {
  const out: { value: string; label: string }[] = [];
  const seen = new Set<string>();
  const add = (value: string, label: string) => {
    if (!value || seen.has(value)) return;
    seen.add(value);
    out.push({ value, label });
  };
  if (workspaceZone) add(workspaceZone, workspaceName ? `${workspaceZone} · ${workspaceName}` : workspaceZone);
  const browser = browserTimeZone();
  if (browser) add(browser, `${browser} · this browser`);
  add(current, current);
  for (const z of allZones()) add(z, z);
  return out;
}

const pad = (n: number) => String(n).padStart(2, "0");

/** 00:00 … 23:45 in quarter hours, plus the saved time when it is off the grid. */
function timeOptions(hour: number, minute: number): { value: string; label: string }[] {
  const out: { value: string; label: string }[] = [];
  for (let h = 0; h < 24; h++) for (const m of [0, 15, 30, 45]) out.push({ value: `${pad(h)}:${pad(m)}`, label: `at ${pad(h)}:${pad(m)}` });
  const cur = `${pad(hour)}:${pad(minute)}`;
  if (!out.some((o) => o.value === cur)) {
    out.push({ value: cur, label: `at ${cur}` });
    out.sort((a, b) => a.value.localeCompare(b.value));
  }
  return out;
}

/**
 * (P5-REFINE-14, schedules-1, -2, -5; D-405) The schedule form: Name, Prompt, **When** (presets from Every 5 minutes, a time
 * picker, the workspace's time zone) and **Advanced**. There is no preset shorter than five minutes; Custom keeps the
 * 5-field cron for the Expert altitude and flags a too-frequent one as you type. The API stays the authority.
 */
export function ScheduleForm({
  agentId,
  initial,
  defaultTimezone,
  workspaceName,
  canOwn,
  embedded = false,
  runAgain = false,
  onDone,
}: {
  agentId: string;
  initial?: Schedule;
  /** (P5-SCHED-ONCE) **Run again** on a Done schedule: the form opens on Once with a new date (tomorrow). */
  runAgain?: boolean;
  /** Inside a row (Edit in place): no card of its own. */
  embedded?: boolean;
  /** The workspace's time zone: the form's default. */
  defaultTimezone?: string;
  workspaceName?: string;
  /** Workspace owners may anchor a new schedule to the workspace. */
  canOwn?: boolean;
  onDone: () => void;
}) {
  const create = useCreateSchedule(agentId);
  const update = useUpdateSchedule(agentId);
  const alt = useAltitude();
  const { setMore, pending: morePending } = useMoreControls();
  const [name, setName] = useState(initial?.name ?? "");
  const [prompt, setPrompt] = useState(initial?.prompt ?? "");
  const [timezone, setTimezone] = useState(initial?.timezone ?? defaultTimezone ?? "UTC");
  const [spec, setSpec] = useState<ScheduleSpec>(() => {
    if (!initial) return { ...DEFAULT_SPEC, preset: "daily" };
    // A one-time schedule opens on Once (Run again: on a new date); anything else is read back from its cron.
    if (!initial.cron && initial.run_at) {
      const once = runAtToSpec(initial.run_at);
      return runAgain || initial.completed_at ? { ...once, date: tomorrowIn(initial.timezone) } : once;
    }
    return cronToSpec(initial.cron);
  });
  const [customCron, setCustomCron] = useState(initial?.cron ?? "");
  const [gateCommand, setGateCommand] = useState(initial?.gate_command ?? "");
  const [gateDescription, setGateDescription] = useState(initial?.gate_description ?? "");
  const [anchored, setAnchored] = useState(false);
  const mut = initial ? update : create;
  const fe = fieldErrors(mut.error);

  const customAllowed = alt.allows("expert");
  const once = spec.preset === "once";
  const cron = once ? "" : spec.preset === "custom" ? customCron.trim() : (specToCron(spec) ?? "");
  const runAt = once ? specToRunAt(spec) : "";
  const tooOften = spec.preset === "custom" && isTooFrequent(customCron);
  const customIncomplete = spec.preset === "custom" && cronFieldCount(customCron) !== 5;

  // The API previews the next three run times; typing waits 1 s and the same cadence is answered from the cache for 60 s.
  // A one-time schedule previews its instant the same way (C-1092): a past time is answered while you type.
  const settled = useDebounced(cron, PREVIEW_DEBOUNCE_MS);
  const settledRunAt = useDebounced(runAt, PREVIEW_DEBOUNCE_MS);
  const previewable = cronFieldCount(settled) === 5 && !(spec.preset === "custom" && isTooFrequent(settled));
  const preview = useSchedulePreview(agentId, previewable ? settled : "", timezone, once ? settledRunAt : "");
  const previewError = spec.preset === "custom" && preview.isError ? fieldErrors(preview.error).cron : undefined;
  const next = !once && preview.data && settled === cron ? nextWords(spec.preset, preview.data.next, timezone) : "";
  // "is in the past" is the API's word; the form says what to do about it.
  const onceMessage = once && preview.isError && settledRunAt === runAt ? fieldErrors(preview.error).run_at : undefined;
  const onceError = onceMessage ? (/past/.test(onceMessage) ? "Pick a time in the future" : onceMessage) : undefined;
  const onceMissing = once && !runAt;

  // Every year names its zone once, in "(Zurich time)" after the next run, as the brief words it.
  const words = spec.preset === "custom" ? "Custom schedule" : once ? "" : spec.preset === "yearly" ? (cadenceWords(cron) ?? "") : whenWords(cron, timezone);
  const city = zoneCity(timezone);
  const summary = once
    ? runAt
      ? `Once, on ${onceLocalWords(runAt)}${city ? ` (${city} time)` : ""}.`
      : ""
    : [spec.preset === "custom" ? null : words, next || null].filter(Boolean).join(" · ");

  const hasGate = !!initial?.gate_command || !!fe.gate_command || !!fe.gate_description;
  const zones = useMemo(() => zoneOptions(defaultTimezone, workspaceName, timezone), [defaultTimezone, workspaceName, timezone]);

  function submit(e: FormEvent) {
    e.preventDefault();
    if ((!cron && !runAt) || tooOften || customIncomplete || onceError) return;
    const gate = { gate_command: gateCommand.trim() || null, gate_description: gateDescription.trim() || null };
    // Exactly one of cron / run_at (C-1092): Once sends the local wall time, the others the cron.
    const when = once ? { run_at: runAt } : { cron };
    if (initial) {
      update.mutate({ id: initial.id, body: { name, prompt, ...when, timezone, ...gate } }, { onSuccess: onDone });
    } else {
      create.mutate({ name, prompt, ...when, timezone, ...gate, ...(anchored ? { anchor: "workspace" } : {}) }, { onSuccess: onDone });
    }
  }

  const setPreset = (preset: SchedulePreset) => {
    // Leaving Custom for a preset, or entering it, keeps what the person typed.
    if (preset === "custom" && !customCron) setCustomCron(specToCron(spec) ?? "");
    // Once starts tomorrow, in the chosen zone.
    setSpec((s) => ({ ...s, preset, date: preset === "once" && !s.date ? tomorrowIn(timezone) : s.date }));
  };
  const showTime = spec.preset === "daily" || spec.preset === "weekdays" || spec.preset === "weekly" || spec.preset === "yearly" || once;
  const cronError = fe.cron ?? previewError;

  return (
    <form onSubmit={submit} aria-label={initial ? "Edit schedule" : "New schedule"} className={embedded ? "space-y-4" : "space-y-4 rounded-card border border-line bg-surface p-4 shadow-card sm:p-5"}>
      <Field id="sch-name" label="Name" error={fe.name}>
        <input id="sch-name" className={inputClass} value={name} required placeholder="Morning summary" onChange={(e) => setName(e.target.value)} />
      </Field>
      <Field id="sch-prompt" label="Prompt" error={fe.prompt}>
        <textarea
          id="sch-prompt"
          className={inputClass}
          rows={3}
          value={prompt}
          required
          placeholder="Summarise what changed overnight and tell me what needs me."
          onChange={(e) => setPrompt(e.target.value)}
        />
      </Field>

      <div className="space-y-2">
        <span className="block text-sm font-medium text-ink">When</span>
        <Segmented
          label="When"
          className="flex-wrap"
          value={spec.preset}
          onChange={setPreset}
          options={PRESETS.map((p) => ({ value: p.value, label: p.label, disabled: p.value === "custom" && !customAllowed && spec.preset !== "custom" }))}
        />
        {!customAllowed && spec.preset !== "custom" ? (
          <p className="text-xs text-muted">
            Custom needs more controls ·{" "}
            <button type="button" disabled={morePending} className="min-h-9 text-accent-ink underline underline-offset-2 hover:no-underline" onClick={() => setMore(true)}>
              Show
            </button>
          </p>
        ) : null}

        <div className="flex flex-wrap items-center gap-2">
          {spec.preset === "weekly" ? (
            <Select
              aria-label="Day of the week"
              className="w-auto"
              value={String(spec.weekday)}
              onChange={(v) => setSpec((s) => ({ ...s, weekday: Number(v) }))}
              options={[1, 2, 3, 4, 5, 6, 0].map((d) => ({ value: String(d), label: `on ${WEEKDAY_NAMES[d]}` }))}
            />
          ) : null}
          {once ? (
            <input
              type="date"
              aria-label="Date"
              className={`${inputClass} w-auto`}
              value={spec.date}
              required
              aria-invalid={!!(onceError ?? fe.run_at) || undefined}
              onChange={(e) => setSpec((s) => ({ ...s, date: e.target.value }))}
            />
          ) : null}
          {spec.preset === "yearly" ? (
            <>
              <Select
                aria-label="Day of the month"
                className="w-auto"
                value={String(spec.day)}
                onChange={(v) => setSpec((s) => ({ ...s, day: Number(v) }))}
                options={Array.from({ length: 31 }, (_, i) => ({ value: String(i + 1), label: `on ${i + 1}` }))}
              />
              <Select
                aria-label="Month"
                className="w-auto"
                value={String(spec.month)}
                onChange={(v) => setSpec((s) => ({ ...s, month: Number(v) }))}
                options={MONTH_NAMES.map((m, i) => ({ value: String(i + 1), label: m }))}
              />
            </>
          ) : null}
          {showTime ? (
            <Select
              aria-label="Time of day"
              className="w-auto"
              value={clock(spec.hour, spec.minute)}
              onChange={(v) => setSpec((s) => ({ ...s, hour: Number(v.slice(0, 2)), minute: Number(v.slice(3)) }))}
              options={timeOptions(spec.hour, spec.minute)}
            />
          ) : null}
          {spec.preset === "hourly" ? (
            <Select
              aria-label="Minute of the hour"
              className="w-auto"
              value={String(spec.minute)}
              onChange={(v) => setSpec((s) => ({ ...s, minute: Number(v) }))}
              options={Array.from({ length: 60 }, (_, m) => ({ value: String(m), label: `at minute ${m}` }))}
            />
          ) : null}
          {spec.preset !== "every5" && spec.preset !== "hourly" ? (
            <Combobox aria-label="Time zone" wrapperClassName="w-72 max-w-full" value={timezone} onChange={setTimezone} options={zones} />
          ) : null}
        </div>
        {fe.timezone ? (
          <p role="alert" className="text-xs text-danger-ink">
            {fe.timezone}
          </p>
        ) : null}

        {spec.preset === "custom" ? (
          customAllowed ? (
            <Altitude id="auto.cron" level="expert">
              <Field id="sch-cron" label="Cron (5 fields)" error={cronError} hint="Minute, hour, day, month, weekday. For example 0 9 * * 1-5.">
                <input
                  id="sch-cron"
                  className={`${inputClass} font-mono`}
                  value={customCron}
                  required
                  spellCheck={false}
                  aria-invalid={tooOften || undefined}
                  aria-describedby={tooOften ? "sch-cron-often" : undefined}
                  onChange={(e) => setCustomCron(e.target.value)}
                />
              </Field>
              {tooOften ? (
                <p id="sch-cron-often" role="alert" className="mt-1 text-xs text-danger-ink">
                  Too often: a schedule can run at most every {MIN_GAP_MINUTES} minutes. Try */5 * * * *.
                </p>
              ) : null}
            </Altitude>
          ) : (
            <p className="text-sm text-ink-2">Custom schedule</p>
          )
        ) : null}

        {once && (onceError ?? fe.run_at) ? (
          <p role="alert" className="text-xs text-danger-ink" data-testid="once-error">
            {onceError ?? fe.run_at}
          </p>
        ) : null}

        <p aria-live="polite" className="text-sm text-ink-2" data-testid="schedule-summary">
          {summary ? `${summary} ` : ""}
          {once ? "" : `Schedules run at most every ${MIN_GAP_MINUTES} minutes.`}
        </p>
      </div>

      {/* (P5-REFINE-14) Set up: the check that decides whether the agent is woken, and who keeps the schedule alive. */}
      <Altitude id="auto.advanced" level="setup" promote={hasGate}>
        <details className="group rounded-control border border-line" open={hasGate || undefined}>
          <summary className="min-h-10 cursor-pointer px-3 py-2 text-sm font-medium text-ink">Advanced</summary>
          <div className="space-y-4 border-t border-line p-3">
            <Field
              id="sch-gate"
              label="Only wake when a check finds something"
              error={fe.gate_command}
              hint="Dani is only woken when this command reports something to do."
            >
              <input id="sch-gate" className={`${inputClass} font-mono`} value={gateCommand} placeholder="A command to run before each run" onChange={(e) => setGateCommand(e.target.value)} />
            </Field>
            {gateCommand.trim() || initial?.gate_description ? (
              <Field id="sch-gate-desc" label="What the check looks for" error={fe.gate_description}>
                <input id="sch-gate-desc" className={inputClass} value={gateDescription} onChange={(e) => setGateDescription(e.target.value)} />
              </Field>
            ) : null}
            {!initial && canOwn ? (
              <CheckboxField
                id="sch-anchored"
                label="Keep running without you"
                hint="The schedule keeps running even if you leave the workspace."
                checked={anchored}
                onChange={setAnchored}
              />
            ) : null}
          </div>
        </details>
      </Altitude>

      {mut.error && Object.keys(fe).length === 0 ? <ErrorNotice error={mut.error} /> : null}
      <FormActions className="gap-2">
        <Button type="submit" disabled={mut.isPending || tooOften || customIncomplete || onceMissing || !!onceError}>
          {initial ? "Save" : "Create schedule"}
        </Button>
        <Button type="button" variant="outline" onClick={onDone}>
          Cancel
        </Button>
      </FormActions>
    </form>
  );
}
