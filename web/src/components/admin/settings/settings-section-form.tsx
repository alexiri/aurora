"use client";

import { useQueryClient } from "@tanstack/react-query";
import { useEffect, useState, type FormEvent, type ReactNode } from "react";

import {
  ChoiceListEditor,
  CurrentValues,
  HostListEditor,
  StringListEditor,
  UnitNumberControl,
} from "@/components/admin/settings/settings-field-controls";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { ChoiceCards } from "@/components/ui/choice-cards";
import { ConfirmDialog } from "@/components/ui/confirm-dialog";
import { inputClass } from "@/components/ui/field";
import { SecretInput } from "@/components/ui/secret-input";
import { LockPill } from "@/components/ui/lock-pill";
import { ErrorNotice, Notice } from "@/components/ui/notice";
import { SaveBar, SAVED_MS } from "@/components/ui/save-bar";
import { ItemList, ListRow } from "@/components/ui/section";
import { Select } from "@/components/ui/select";
import { SettingField, SettingsCard, settingControlId } from "@/components/ui/setting-field";
import { Switch } from "@/components/ui/switch";
import { ApiError } from "@/lib/api/client";
import { errorMessage, fieldErrors } from "@/lib/api/errors";
import { defaultInWords, valueWords } from "@/lib/settings-words";
import {
  useSettingsSaver,
  type FieldInfo,
  type Setting,
  type SettingsPreview,
  type SettingsSection,
  type SettingValues,
} from "@/lib/api/settings";

/** What a section page's own control for one setting is given (SET-3: the LLM page's editors). */
export interface SettingControlCtx {
  setting: Setting;
  /** The value being edited: the draft, else the effective value (a dashboard-less key shows its default). */
  current: unknown;
  /** Has the form changed this key (a dashboard value is pending or in force)? */
  hasValue: boolean;
  /** The setting's own error, and the errors of its entries (`<key>/<id>/<field>`). */
  error?: string;
  entryErrors: [string, string][];
  /** The dashboard entries of a keyed list being edited (Helm entries are `setting.entries`). */
  own: Record<string, unknown>[];
  /** Every pending dashboard value of the section (a control that depends on another key reads it here). */
  values: Record<string, unknown>;
  setValue: (key: string, v: unknown) => void;
  resetValue: (key: string) => void;
}

/** A titled group of a section's settings (`keys: null` = all of them). */
export interface SettingsGroup {
  title: string;
  description?: string;
  keys: string[] | null;
}

type SecretMode = "keep" | "replace" | "remove";
interface SecretDraft {
  mode: SecretMode;
  value: string;
}

/** Everything the form edits: the dashboard values (absent key = default) and the secret fields. */
interface Draft {
  values: Record<string, unknown>;
  secrets: Record<string, SecretDraft>;
}

function isSecretMarker(v: unknown): v is { $secret: unknown } {
  return typeof v === "object" && v !== null && !Array.isArray(v) && "$secret" in v;
}

/** Secrets inside stored entries come back as `{ $secret: { set } }`: send them back as `keep`. */
function toWire(v: unknown): unknown {
  // A stored marker has an object (`{ set }`); a plaintext the admin just typed (`{ $secret: "…" }`) goes as typed.
  if (isSecretMarker(v)) return typeof v.$secret === "object" ? { $secret: "keep" } : v;
  if (Array.isArray(v)) return v.map(toWire);
  if (typeof v === "object" && v !== null) {
    return Object.fromEntries(Object.entries(v).map(([k, x]) => [k, toWire(x)]));
  }
  return v;
}

function initialDraft(section: SettingsSection): Draft {
  const values: Record<string, unknown> = {};
  const secrets: Record<string, SecretDraft> = {};
  for (const s of section.settings) {
    if (s.secret) {
      if (s.secret.set) secrets[s.key] = { mode: "keep", value: "" };
      continue;
    }
    if (s.locked) continue;
    if (s.entries) {
      const mine = s.entries.filter((e) => e.source === "dashboard").map((e) => e.value);
      if (mine.length > 0) values[s.key] = mine;
    } else if (s.source === "dashboard") {
      values[s.key] = s.value;
    }
  }
  return { values, secrets };
}

/** The PUT body's `values` (C-773): dashboard values, secrets as `{ $secret }` markers. */
function toValues(section: SettingsSection, d: Draft): SettingValues {
  const out: SettingValues = {};
  for (const s of section.settings) {
    if (s.locked && !s.entries) continue;
    if (s.secret) {
      const sd = d.secrets[s.key];
      if (!sd || sd.mode === "remove") continue;
      out[s.key] = { $secret: sd.mode === "keep" ? "keep" : sd.value };
      continue;
    }
    if (s.key in d.values) out[s.key] = toWire(d.values[s.key]);
  }
  return out;
}

/** A locked setting's value, in words (never a raw id or JSON). */
function formatValue(s: Setting, field: FieldInfo | undefined): ReactNode {
  const v = s.value as unknown;
  if (Array.isArray(v) && v.length > 0 && field?.kind !== "keyed_list") {
    return v.map((x, i) => <div key={i}>{valueWords(field, [x])}</div>);
  }
  const text = valueWords(field, v);
  return text === "Not set" || text === "None" ? <span className="text-muted">{text}</span> : text;
}

/** Save needs a confirmation when the preview reports loosening, affected organizations or warnings. */
function needsConfirmation(p: SettingsPreview): boolean {
  return p.loosening.length > 0 || p.affected_orgs_total > 0 || p.warnings.length > 0;
}

/**
 * The generic section form (SET-1, P5-IA-5): renders any C-772 section from its `fields` (C-972) — a label, one
 * sentence of help, choice cards, list editors, numbers with units and a "Default: …" badge in words — through
 * `SettingField`. A section that is not editable yet (C-973) shows its current values read-only instead, with no
 * Save. Save calls the preview route first and asks for confirmation when something loosens; 409 / 422 follow
 * `14 §Instance administration`.
 */
export function SettingsSectionForm(props: {
  section: SettingsSection;
  /** Extra content above Save, given the values a save would send (SET-4: the e-mail page's test button). */
  footer?: (values: SettingValues, dirty: boolean) => ReactNode;
  /** Refetch the section (stale revision / locked key). */
  onReload: () => void;
  /** Split the settings into titled `FormSection`s (a section page's own layout); default: one. */
  groups?: SettingsGroup[];
  /** A section page's own control for a key, replacing the generic one (the save pipeline stays shared). */
  controls?: Record<string, (ctx: SettingControlCtx) => ReactNode>;
}) {
  if (props.section.editable === false) {
    const keys = props.groups?.flatMap((g) => g.keys ?? []);
    const shown = (props.section.current ?? []).filter((c) => !keys || keys.includes(c.key));
    return <CurrentValues title={props.section.title} current={shown} fields={props.section.fields ?? []} />;
  }
  return (
    <SettingsStackForm
      parts={[{ section: props.section, groups: props.groups, controls: props.controls }]}
      onReload={props.onReload}
      footer={props.footer}
    />
  );
}

/** One section of a stacked page: its settings laid out in titled groups, and whatever sits beneath them. */
export interface StackPart {
  section: SettingsSection;
  groups?: SettingsGroup[];
  controls?: Record<string, (ctx: SettingControlCtx) => ReactNode>;
  /** A notice above this section's groups. */
  above?: ReactNode;
  /** Content between this section's groups and the next section (a read-only table). */
  below?: ReactNode;
}

/** One draft per section, kept only for the generation it was made on (a reload or a save drops it). */
type Drafts = Record<string, { generation: number; draft: Draft }>;

/**
 * One form for one or more sections (P5-IA-3): each section keeps its own draft, preview and PUT (the API saves per
 * section), but the page has one Save, one confirmation listing what every changed section loosens, and one Discard.
 * A Helm-locked key is never part of a draft, so it stays read-only. If a section fails, the ones saved before it stay
 * saved and the error appears at the failing section.
 */
export function SettingsStackForm({
  parts,
  onReload,
  footer,
}: {
  parts: StackPart[];
  onReload: () => void;
  /** Extra content above Save, given the values the first section would send (SET-4: the e-mail page's test button). */
  footer?: (values: SettingValues, dirty: boolean) => ReactNode;
}) {
  const [drafts, setDrafts] = useState<Drafts>({});
  const [errors, setErrors] = useState<Record<string, unknown>>({});
  const [saved, setSaved] = useState(false);
  // "Saved" stays for two seconds (and its Undo with it).
  useEffect(() => {
    if (!saved) return;
    const t = setTimeout(() => setSaved(false), SAVED_MS);
    return () => clearTimeout(t);
  }, [saved]);
  const [confirm, setConfirm] = useState<{ part: StackPart; preview: SettingsPreview }[] | null>(null);
  const [busy, setBusy] = useState(false);
  const saver = useSettingsSaver();
  const qc = useQueryClient();

  const states = parts.map((part) => {
    const initial = initialDraft(part.section);
    const mine = drafts[part.section.section];
    const draft = mine && mine.generation === part.section.generation ? mine.draft : initial;
    const dirty = JSON.stringify(toValues(part.section, draft)) !== JSON.stringify(toValues(part.section, initial));
    return { part, initial, draft, dirty };
  });
  const dirty = states.some((s) => s.dirty);
  const stacked = parts.length > 1;

  function edit(part: StackPart, change: (d: Draft) => Draft) {
    setSaved(false);
    setDrafts((all) => {
      const id = part.section.section;
      const cur = all[id] && all[id].generation === part.section.generation ? all[id].draft : initialDraft(part.section);
      return { ...all, [id]: { generation: part.section.generation, draft: change(cur) } };
    });
  }

  /** Helm changed under the page: show what is locked now. */
  function lockedOut() {
    void qc.invalidateQueries({ queryKey: ["admin", "settings"] });
    onReload();
  }

  /** What a save replaced, so **Undo** can put it back: each section's saved values before the change. */
  const [before, setBefore] = useState<{ section: string; values: SettingValues }[] | null>(null);
  const undoable = before !== null && saved;
  async function undo() {
    if (!before) return;
    setBusy(true);
    try {
      for (const b of before) {
        const part = parts.find((p) => p.section.section === b.section);
        if (!part) continue;
        await saver.put(b.section, { generation: part.section.generation, values: b.values });
      }
      setBefore(null);
      setSaved(false);
      setErrors({});
    } catch (e) {
      setErrors({ [before[0]?.section ?? ""]: e });
    } finally {
      setBusy(false);
    }
  }

  async function doPut(changed: typeof states) {
    const errs: Record<string, unknown> = {};
    let done = 0;
    const snapshot = changed.map((s) => ({ section: s.part.section.section, values: toValues(s.part.section, s.initial) }));
    for (const s of changed) {
      try {
        await saver.put(s.part.section.section, {
          generation: s.part.section.generation,
          values: toValues(s.part.section, s.draft),
        });
        done += 1;
      } catch (e) {
        errs[s.part.section.section] = e;
        if (e instanceof ApiError && e.code === "setting_locked") lockedOut();
        break;
      }
    }
    setConfirm(null);
    setErrors(errs);
    const ok = done > 0 && Object.keys(errs).length === 0;
    setSaved(ok);
    setBefore(ok ? snapshot : null);
  }

  async function submit(e?: FormEvent) {
    e?.preventDefault();
    setErrors({});
    setSaved(false);
    setBusy(true);
    try {
      const changed = states.filter((s) => s.dirty);
      const needing: { part: StackPart; preview: SettingsPreview }[] = [];
      for (const s of changed) {
        try {
          const p = await saver.preview(s.part.section.section, {
            generation: s.part.section.generation,
            values: toValues(s.part.section, s.draft),
          });
          if (needsConfirmation(p)) needing.push({ part: s.part, preview: p });
        } catch (err) {
          setErrors({ [s.part.section.section]: err });
          if (err instanceof ApiError && err.code === "setting_locked") onReload();
          return;
        }
      }
      if (needing.length > 0) setConfirm(needing);
      else await doPut(changed);
    } finally {
      setBusy(false);
    }
  }

  async function confirmed() {
    setBusy(true);
    try {
      await doPut(states.filter((s) => s.dirty));
    } finally {
      setBusy(false);
    }
  }

  // The bar's words: how many fields need fixing, or the one failure that is not a field's (stale revision, locked key).
  let fieldErrorCount = 0;
  let otherError: ReactNode = undefined;
  for (const { part } of states) {
    const err = errors[part.section.section];
    if (!err) continue;
    const known = new Set(part.section.settings.map((s) => s.key));
    const fe = fieldErrors(err);
    const mine = Object.keys(fe).filter((k) => known.has(k.split("/")[0] ?? k));
    fieldErrorCount += mine.length;
    if (mine.length === 0) otherError ??= errorMessage(err);
  }

  return (
    <form onSubmit={(e) => void submit(e)} noValidate className="space-y-5">
      {states.map(({ part, draft }) => {
        const section = part.section;
        const error = errors[section.section];
        const errs = error ? fieldErrors(error) : {};
        const fieldsByKey = new Map((section.fields ?? []).map((f) => [f.key, f]));
        const knownKeys = new Set(section.settings.map((s) => s.key));
        const otherErrs = Object.entries(errs).filter(([k]) => !knownKeys.has(k.split("/")[0] ?? k));
        const code = error instanceof ApiError ? error.code : null;
        const setValue = (key: string, v: unknown) => edit(part, (d) => ({ ...d, values: { ...d.values, [key]: v } }));
        const resetValue = (key: string) =>
          edit(part, (d) => {
            const values = { ...d.values };
            delete values[key];
            return { ...d, values };
          });
        const setSecret = (key: string, sd: SecretDraft | null) =>
          edit(part, (d) => {
            const secrets = { ...d.secrets };
            if (sd) secrets[key] = sd;
            else delete secrets[key];
            return { ...d, secrets };
          });
        return (
          <div key={section.section} className="space-y-5">
            {code === "stale_revision" ? (
              <Notice tone="pending" role="alert" className="space-y-2">
                <p>Someone changed these settings meanwhile. Reload to see their changes; your edits to this part are dropped.</p>
                <Button type="button" size="sm" variant="outline" onClick={onReload}>
                  Reload
                </Button>
              </Notice>
            ) : null}
            {code === "setting_locked" ? (
              <Notice tone="danger" role="alert">
                A setting you changed is configured in Helm and cannot be changed here. The page was reloaded.
              </Notice>
            ) : null}
            {error && !["stale_revision", "setting_locked", "settings_invalid"].includes(code ?? "") ? (
              <ErrorNotice error={error} />
            ) : null}
            {code === "settings_invalid" ? (
              <Notice tone="danger" role="alert">
                <p>{errorMessage(error)}</p>
                {otherErrs.length > 0 ? (
                  <ul className="mt-1 list-disc pl-5">
                    {otherErrs.map(([k, m]) => (
                      <li key={k}>
                        <code className="font-mono">{k}</code>: {m}
                      </li>
                    ))}
                  </ul>
                ) : null}
              </Notice>
            ) : null}

            {part.above ?? null}
            {(part.groups ?? [{ title: section.title, description: section.description, keys: null }]).map((g) => {
              const shown = g.keys ? section.settings.filter((s) => g.keys?.includes(s.key)) : section.settings;
              if (shown.length === 0) return null;
              return (
                <SettingsCard
                  key={g.title}
                  title={g.title}
                  description={g.description}
                  guide={shown.map((s) => fieldsByKey.get(s.key)?.doc?.title).find((t): t is string => Boolean(t))}>
                  {shown.map((s) => (
                    <SettingRow
                      key={s.key}
                      setting={s}
                      field={fieldsByKey.get(s.key)}
                      draft={draft}
                      error={errs[s.key]}
                      entryErrors={Object.entries(errs).filter(([k]) => k.startsWith(`${s.key}/`))}
                      setValue={setValue}
                      resetValue={resetValue}
                      setSecret={setSecret}
                      custom={part.controls?.[s.key]}
                    />
                  ))}
                </SettingsCard>
              );
            })}
            {part.below ?? null}
          </div>
        );
      })}

      {footer && states[0] ? footer(toValues(states[0].part.section, states[0].draft), states[0].dirty) : null}

      {/* (P5-REFINE-2, admin-2) The one save bar: fixed at the bottom of the viewport, so it is always in view on a long page. */}
      <SaveBar
        dirty={dirty}
        saving={busy}
        saved={saved}
        errorCount={fieldErrorCount}
        error={otherError}
        savedMessage="Saved. Applied to every replica within seconds."
        onUndo={undoable ? () => void undo() : undefined}
        onDiscard={() => {
          setDrafts({});
          setErrors({});
          setSaved(false);
        }}
      />

      <ConfirmDialog
        open={confirm !== null}
        onOpenChange={(o) => {
          if (!o) setConfirm(null);
        }}
        title="Save these settings?"
        description="This changes what people and agents are allowed to do."
        confirmLabel="Save"
        busy={busy}
        onConfirm={() => void confirmed()}
      >
        {confirm
          ? confirm.map((c) => (
              <div key={c.part.section.section} className="space-y-1">
                {stacked ? <p className="text-sm font-semibold text-ink">{c.part.section.title}</p> : null}
                <ConfirmDetails preview={c.preview} />
              </div>
            ))
          : null}
      </ConfirmDialog>
    </form>
  );
}

/** LLM keys an organization loses by the change (the `llm.endpoints` change of C-749: usable keys before and after). */
function keysLost(o: SettingsPreview["affected_orgs"][number]): number {
  const c = o.changes.find((x) => x.key === "llm.endpoints");
  const n = (v: unknown) => (typeof v === "object" && v !== null && "usable_keys" in v ? Number((v as { usable_keys: unknown }).usable_keys) : 0);
  return c ? Math.max(0, n(c.before) - n(c.after)) : 0;
}

function ConfirmDetails({ preview }: { preview: SettingsPreview }) {
  return (
    <div className="space-y-3 text-sm">
      {preview.loosening.length > 0 ? (
        <div>
          <p className="font-medium text-ink">This allows more than before:</p>
          <ul className="list-disc pl-5 text-ink-2">
            {preview.loosening.map((l, i) => (
              <li key={i}>
                <code className="font-mono">{l.key}</code>
                {l.entry_id ? <> ({l.entry_id})</> : null}: {l.reason}
              </li>
            ))}
          </ul>
        </div>
      ) : null}
      {preview.affected_orgs_total > 0 ? (
        <div>
          <p className="font-medium text-ink">
            Affects {preview.affected_orgs_total} organization{preview.affected_orgs_total === 1 ? "" : "s"}:
          </p>
          <ul className="list-disc pl-5 text-ink-2">
            {preview.affected_orgs.map((o) => {
              const lost = keysLost(o);
              // (SET-5, C-783) A lowered retention ceiling: the organization's own value and what applies now.
              const clamped = o.changes.filter((c) => c.key === "conversations_days" || c.key === "attachments_days" || c.key === "attachment_quota_mib");
              return (
                <li key={o.org_id}>
                  {o.name}
                  {lost > 0 ? ` — ${lost} LLM key${lost === 1 ? "" : "s"} stop${lost === 1 ? "s" : ""} working` : ""}
                  {clamped.length > 0
                    ? ` — ${clamped.map((c) => `${c.key.replace(/_/g, " ")} ${String(c.before)} → ${String(c.after)}`).join(", ")}`
                    : ""}
                </li>
              );
            })}
          </ul>
          {preview.affected_orgs_total > preview.affected_orgs.length ? (
            <p className="text-xs text-muted">and {preview.affected_orgs_total - preview.affected_orgs.length} more</p>
          ) : null}
        </div>
      ) : null}
      {preview.warnings.length > 0 ? (
        <ul className="list-disc pl-5 text-ink-2">
          {preview.warnings.map((w, i) => (
            <li key={i}>{w.message}</li>
          ))}
        </ul>
      ) : null}
    </div>
  );
}

function SettingRow({
  setting: s,
  field,
  draft,
  error,
  entryErrors,
  setValue,
  resetValue,
  setSecret,
  custom,
}: {
  setting: Setting;
  /** The setting's words (C-972): label, help, choices, default, unit, operator doc. */
  field: FieldInfo | undefined;
  draft: Draft;
  error?: string;
  entryErrors: [string, string][];
  setValue: (key: string, v: unknown) => void;
  resetValue: (key: string) => void;
  setSecret: (key: string, sd: SecretDraft | null) => void;
  custom?: (ctx: SettingControlCtx) => ReactNode;
}) {
  // The registry completeness test keeps every movable key in `fields`; the key is only a last resort for a bare fixture.
  const label = field?.label ?? s.key;
  const hint = field?.help;
  const docTitle = field?.doc?.title;
  const cid = settingControlId(s.key);

  if (custom && !s.secret) {
    const has = s.key in draft.values;
    return (
      <>
        {custom({
          setting: s,
          current: has ? draft.values[s.key] : s.entries ? undefined : s.value,
          hasValue: has,
          error,
          entryErrors,
          own: (draft.values[s.key] as Record<string, unknown>[] | undefined) ?? [],
          values: draft.values,
          setValue,
          resetValue,
        })}
      </>
    );
  }

  if (s.secret) {
    const sd = draft.secrets[s.key];
    const fromEnv = s.secret.from_env;
    if (fromEnv) {
      return (
        <SettingField id={s.key} label={label} source="helm" helmKey={fromEnv} lockedValue="Set from the environment" doc={docTitle} />
      );
    }
    return (
      <SettingField id={s.key} label={label} hint={hint} doc={docTitle} source={sd ? "dashboard" : "default"} error={error}>
        <div id={cid} className="flex flex-wrap items-center gap-2">
          {sd?.mode === "replace" ? (
            <div className="w-full max-w-sm">
              <SecretInput
                aria-label={`${label}: new value`}
                revealLabel={label}
                value={sd.value}
                onChange={(v) => setSecret(s.key, { mode: "replace", value: v })}
              />
            </div>
          ) : (
            <span className="text-sm text-ink-2">{sd?.mode === "keep" ? "Set" : "Not set"}</span>
          )}
          <Button type="button" variant="outline" size="sm" onClick={() => setSecret(s.key, { mode: "replace", value: "" })}>
            {sd ? "Replace…" : "Set…"}
          </Button>
          {sd ? (
            <Button type="button" variant="ghost" size="sm" onClick={() => setSecret(s.key, null)}>
              Remove
            </Button>
          ) : null}
        </div>
      </SettingField>
    );
  }

  if (s.entries) {
    return (
      <KeyedList
        setting={s}
        label={label}
        hint={hint}
        doc={docTitle}
        entries={draft.values[s.key] as Record<string, unknown>[] | undefined}
        error={error}
        entryErrors={entryErrors}
        onChange={(v) => (v.length === 0 ? resetValue(s.key) : setValue(s.key, v))}
      />
    );
  }

  const locked = s.locked;
  const has = s.key in draft.values;
  const current = has ? draft.values[s.key] : (s.default as unknown);
  const source = locked ? "helm" : has ? "dashboard" : "default";
  const kind = field?.kind;
  let control: ReactNode;
  if (kind === "enum" && field?.choices) {
    // An enumeration is a choice among sentences, never a raw id (D-341). The label above is the legend.
    control = (
      <div id={cid}>
        <ChoiceCards
          legend={label}
          legendHidden
          value={String(current ?? "")}
          options={field.choices.map((c) => ({ value: c.value, label: c.label, description: c.description }))}
          onChange={(v) => setValue(s.key, v)}
        />
      </div>
    );
  } else if (kind === "bool" || (!field && s.schema.kind === "bool")) {
    control = (
      <Switch id={cid} aria-label={label} checked={current === true} onCheckedChange={(v) => setValue(s.key, v)} />
    );
  } else if (s.schema.kind === "enum") {
    control = (
      <Select id={cid} value={String(current ?? "")} onChange={(v) => setValue(s.key, v)} options={(s.schema.options ?? []).map((o) => ({ value: o, label: o }))} />
    );
  } else if (field && (kind === "int" || kind === "float" || kind === "duration")) {
    // A decimal setting (the rate multiplier) offers no lower bound: the schema's bounds are whole numbers and the
    // server names the exact one.
    control = (
      <UnitNumberControl
        id={cid}
        field={field}
        value={current}
        min={s.schema.min}
        max={s.schema.max}
        decimal={kind === "float"}
        onChange={(v) => setValue(s.key, v)}
        onClear={() => resetValue(s.key)}
      />
    );
  } else if (s.schema.kind === "int" || s.schema.kind === "float" || s.schema.kind === "duration_secs") {
    control = (
      <input
        id={cid}
        type="number"
        step={s.schema.kind === "float" ? "any" : undefined}
        className={`${inputClass} max-w-48`}
        value={current === null || current === undefined ? "" : String(current)}
        onChange={(e) => (e.target.value === "" ? resetValue(s.key) : setValue(s.key, Number(e.target.value)))}
      />
    );
  } else if (s.schema.kind === "string") {
    const long = (s.schema.max_len ?? 0) > 500;
    const common = {
      id: cid,
      className: inputClass,
      maxLength: s.schema.max_len ?? undefined,
      value: typeof current === "string" ? current : "",
    };
    control = long ? (
      <textarea {...common} rows={6} onChange={(e) => setValue(s.key, e.target.value)} />
    ) : (
      <input {...common} onChange={(e) => setValue(s.key, e.target.value)} />
    );
  } else if (s.schema.kind === "list" && field?.choices) {
    control = <ChoiceListEditor id={cid} field={field} value={current} onChange={(v) => setValue(s.key, v)} />;
  } else if (s.schema.kind === "list" && s.schema.item_kind === "string") {
    control = <StringListEditor id={cid} label={label} value={current} onChange={(v) => setValue(s.key, v)} />;
  } else if (s.schema.kind === "list" && s.key === "governance.egress.baseline_allowlist") {
    control = <HostListEditor id={cid} label={label} value={current} onChange={(v) => setValue(s.key, v)} />;
  } else {
    // A list this page has no editor for: its entries in words, read-only.
    control = (
      <p id={cid} className="text-sm text-ink">
        {valueWords(field, current)}
      </p>
    );
  }
  return (
    <SettingField
      id={s.key}
      label={label}
      hint={hint}
      source={source}
      helmKey={s.helm_key}
      lockedValue={formatValue(s, field)}
      defaultLabel={field ? defaultInWords(field.default_display) : undefined}
      doc={docTitle}
      onReset={() => resetValue(s.key)}
      error={error}
    >
      {control}
    </SettingField>
  );
}

/**
 * A keyed list no page has an editor for (every real one has: sign-in providers, AI endpoints, secret stores, rate
 * classes): the Helm entries are locked ("Managed by Helm"), the others can be removed. Entries are never typed as JSON.
 */
function KeyedList({
  setting: s,
  label,
  hint,
  doc,
  entries: mine,
  error,
  entryErrors,
  onChange,
}: {
  setting: Setting;
  label: string;
  hint?: string;
  doc?: string;
  entries: Record<string, unknown>[] | undefined;
  error?: string;
  entryErrors: [string, string][];
  onChange: (v: Record<string, unknown>[]) => void;
}) {
  const idField = s.id_field ?? "id";
  const own = mine ?? [];
  const helm = (s.entries ?? []).filter((e) => e.source === "helm");
  const idError = entryErrors.find(([, m]) => m)?.[1];

  return (
    <SettingField
      id={s.key}
      label={label}
      hint={hint}
      doc={doc}
      source={own.length > 0 ? "dashboard" : "default"}
      defaultLabel="None"
      error={error ?? idError}
    >
      <div id={settingControlId(s.key)} className="space-y-2">
        <ItemList aria-label={label}>
          {helm.map((e) => (
            <ListRow key={`h-${e.id}`}>
              <span className="font-medium text-ink">{e.id}</span>
              <LockPill owner="Set in Helm" />
            </ListRow>
          ))}
          {own.map((e, i) => (
            <ListRow
              key={`d-${String(e[idField])}-${i}`}
              actions={
                <Button type="button" variant="ghost" size="sm" onClick={() => onChange(own.filter((_, j) => j !== i))}>
                  Remove
                </Button>
              }
            >
              <span className="font-medium text-ink">{String(e[idField])}</span>
              <Badge tone="accent">Dashboard</Badge>
            </ListRow>
          ))}
          {helm.length + own.length === 0 ? <ListRow>None yet.</ListRow> : null}
        </ItemList>
      </div>
    </SettingField>
  );
}
