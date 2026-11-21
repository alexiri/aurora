"use client";

import { useState, type ReactNode } from "react";

import type { SettingControlCtx } from "@/components/admin/settings/settings-section-form";
import { inputClass } from "@/components/ui/field";
import { CheckboxField } from "@/components/ui/form";
import { Notice } from "@/components/ui/notice";
import { SettingField, settingControlId } from "@/components/ui/setting-field";
import type { FieldInfo, SettingsSection } from "@/lib/api/settings";

export const ENABLED_HARNESSES = "hosted.enabled_harnesses";
export const DEFAULT_HARNESS = "hosted.default_harness";

const strs = (v: unknown): string[] => (Array.isArray(v) ? v.filter((x): x is string => typeof x === "string") : []);

export interface HarnessChoice {
  value: string;
  label: string;
  description: string;
}

/** The default that a tick moved, for the hint beside the default's select until the form is saved or reset. */
export interface MovedDefault {
  from: string;
  to: string;
}

/** The harness choices of a field, in the server's order. */
export function harnessChoices(fields: FieldInfo[] | null | undefined): HarnessChoice[] {
  return (fields ?? []).find((f) => f.key === ENABLED_HARNESSES)?.choices ?? [];
}

/** The display name of a harness; an id that is not a choice shows as it is. */
export function harnessName(choices: HarnessChoice[], id: string): string {
  return choices.find((c) => c.value === id)?.label ?? id;
}

/** `agents.extra.offered_harnesses`: the ids registered runners offer, or null while none is registered. */
export function offeredHarnesses(section: SettingsSection): string[] | null {
  const extra = section.extra as { offered_harnesses?: string[] | null } | null | undefined;
  return Array.isArray(extra?.offered_harnesses) ? extra.offered_harnesses : null;
}

/** Holds the moved-default note for the two controls, which are separate settings (D-374). */
export function useMovedDefault() {
  return useState<MovedDefault | null>(null);
}

/** The two harness controls of Administration › What agents may do › Defaults, given the section they belong to. */
export function harnessControls(
  section: SettingsSection,
  moved: MovedDefault | null,
  setMoved: (m: MovedDefault | null) => void,
): Record<string, (ctx: SettingControlCtx) => ReactNode> {
  const choices = harnessChoices(section.fields);
  const offered = offeredHarnesses(section);
  const def = section.settings.find((s) => s.key === DEFAULT_HARNESS);
  return {
    [ENABLED_HARNESSES]: (ctx) => (
      <EnabledHarnessesControl
        ctx={ctx}
        choices={choices}
        offered={offered}
        defaultStored={typeof def?.value === "string" ? def.value : ""}
        defaultLocked={def?.locked === true}
        setMoved={setMoved}
      />
    ),
    [DEFAULT_HARNESS]: (ctx) => (
      <DefaultHarnessControl
        ctx={ctx}
        choices={choices}
        enabledStored={strs(section.settings.find((s) => s.key === ENABLED_HARNESSES)?.value)}
        moved={moved}
        setMoved={setMoved}
      />
    ),
  };
}

/** The programs agents may run, ticked by name; only what a registered runner offers can be newly ticked. */
export function EnabledHarnessesControl({
  ctx,
  choices,
  offered,
  defaultStored,
  defaultLocked,
  setMoved,
}: {
  ctx: SettingControlCtx;
  choices: HarnessChoice[];
  offered: string[] | null;
  defaultStored: string;
  defaultLocked: boolean;
  setMoved: (m: MovedDefault | null) => void;
}) {
  const s = ctx.setting;
  const enabled = strs(ctx.current);
  const cid = settingControlId(s.key);
  if (s.locked) {
    return (
      <SettingField
        id={s.key}
        label="Programs agents may run"
        source="helm"
        helmKey={s.helm_key}
        lockedValue={enabled.length ? enabled.map((id) => harnessName(choices, id)).join(", ") : "None"}
      />
    );
  }
  const known = new Set(choices.map((c) => c.value));
  const others = enabled.filter((id) => !known.has(id));
  function toggle(id: string, on: boolean) {
    const kept = choices.map((c) => c.value).filter((v) => (v === id ? on : enabled.includes(v)));
    // Values that are not choices (`fake` on a development instance) are kept, after the chosen ones.
    const next = [...kept, ...others];
    ctx.setValue(ENABLED_HARNESSES, next);
    // The default must stay among the ticked programs: when a tick removes it, it moves to the first still ticked.
    const current = typeof ctx.values[DEFAULT_HARNESS] === "string" ? (ctx.values[DEFAULT_HARNESS] as string) : defaultStored;
    if (!on && !defaultLocked && current === id && next.length > 0) {
      ctx.setValue(DEFAULT_HARNESS, next[0]!);
      setMoved({ from: id, to: next[0]! });
    }
  }
  return (
    <SettingField
      id={s.key}
      label="Programs agents may run"
      hint="Tick the programs agents may run. Only programs a registered runner offers can be ticked."
      source={ctx.hasValue ? "dashboard" : "default"}
      defaultLabel={strs(s.default).map((id) => harnessName(choices, id)).join(", ")}
      onReset={() => ctx.resetValue(s.key)}
      error={ctx.error}
    >
      <div id={cid} role="group" aria-label="Programs agents may run" className="space-y-2">
        {offered === null ? (
          <Notice role="status">No runner is registered yet, so which programs it offers can&apos;t be checked.</Notice>
        ) : null}
        {choices.map((c) => {
          const on = enabled.includes(c.value);
          const missing = offered !== null && !offered.includes(c.value);
          return (
            <CheckboxField
              key={c.value}
              id={`harness-${c.value}`}
              label={c.label}
              hint={
                missing ? (
                  <>
                    {c.description} <span className={on ? "font-medium text-pending-ink" : undefined}>No registered runner offers this.</span>
                  </>
                ) : (
                  c.description
                )
              }
              checked={on}
              disabled={missing && !on}
              onChange={(next) => toggle(c.value, next)}
            />
          );
        })}
        {others.length > 0 ? <p className="text-sm text-ink-2">Also enabled: {others.join(", ")}</p> : null}
      </div>
    </SettingField>
  );
}

/** The program new agents use: a select of the ticked programs. */
export function DefaultHarnessControl({
  ctx,
  choices,
  enabledStored,
  moved,
  setMoved,
}: {
  ctx: SettingControlCtx;
  choices: HarnessChoice[];
  /** The enabled list as stored, for a form that has not touched the list. */
  enabledStored: string[];
  moved: MovedDefault | null;
  setMoved: (m: MovedDefault | null) => void;
}) {
  const s = ctx.setting;
  const current = typeof ctx.current === "string" ? ctx.current : "";
  const cid = settingControlId(s.key);
  if (s.locked) {
    return (
      <SettingField
        id={s.key}
        label="Program new agents use"
        source="helm"
        helmKey={s.helm_key}
        lockedValue={harnessName(choices, current)}
      />
    );
  }
  const list = ENABLED_HARNESSES in ctx.values ? strs(ctx.values[ENABLED_HARNESSES]) : enabledStored;
  const options = choices.filter((c) => list.includes(c.value)).map((c) => ({ id: c.value, name: c.label }));
  // A default that is not a choice (`fake`) is its own option: it is never rewritten silently.
  if (current !== "" && !options.some((o) => o.id === current)) options.push({ id: current, name: harnessName(choices, current) });
  const note =
    moved && ctx.hasValue && current === moved.to
      ? `Changed to ${harnessName(choices, moved.to)} because ${harnessName(choices, moved.from)} is no longer ticked.`
      : null;
  return (
    <SettingField
      id={s.key}
      label="Program new agents use"
      hint={note ?? "The program a new agent starts with. It must be one of the ticked programs."}
      source={ctx.hasValue ? "dashboard" : "default"}
      defaultLabel={harnessName(choices, typeof s.default === "string" ? s.default : "")}
      onReset={() => {
        setMoved(null);
        ctx.resetValue(s.key);
      }}
      error={ctx.error}
    >
      <select
        id={cid}
        className={`${inputClass} max-w-sm`}
        value={current}
        onChange={(e) => {
          setMoved(null);
          ctx.setValue(DEFAULT_HARNESS, e.target.value);
        }}
      >
        {options.map((o) => (
          <option key={o.id} value={o.id}>
            {o.name}
          </option>
        ))}
      </select>
    </SettingField>
  );
}
