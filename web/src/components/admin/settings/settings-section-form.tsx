"use client";

import { useQueryClient } from "@tanstack/react-query";
import { useMemo, useState, type FormEvent, type ReactNode } from "react";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { ConfirmDialog, FormDialog } from "@/components/ui/confirm-dialog";
import { inputClass } from "@/components/ui/field";
import { FormActions, FormSection } from "@/components/ui/form";
import { Notice } from "@/components/ui/notice";
import { ItemList, ListRow } from "@/components/ui/section";
import { SettingField, settingControlId } from "@/components/ui/setting-field";
import { ApiError } from "@/lib/api/client";
import { errorMessage, fieldErrors } from "@/lib/api/errors";
import {
  usePreviewSettings,
  usePutSettings,
  type Setting,
  type SettingsPreview,
  type SettingsSection,
  type SettingValues,
} from "@/lib/api/settings";

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
  if (isSecretMarker(v)) return { $secret: "keep" };
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

function formatValue(s: Setting): ReactNode {
  const v = s.value as unknown;
  if (v === null || v === undefined) return <span className="text-muted">Not set</span>;
  if (typeof v === "boolean") return v ? "On" : "Off";
  if (Array.isArray(v)) {
    if (v.length === 0) return <span className="text-muted">Empty list</span>;
    return v.map((x, i) => (
      <div key={i}>{typeof x === "string" ? x : <code className="font-mono text-xs">{JSON.stringify(x)}</code>}</div>
    ));
  }
  if (typeof v === "object") return <code className="font-mono text-xs">{JSON.stringify(v)}</code>;
  return String(v);
}

function defaultText(s: Setting): string | undefined {
  const d = s.default as unknown;
  if (d === null || d === undefined) return undefined;
  if (typeof d === "boolean") return d ? "on" : "off";
  if (Array.isArray(d)) return d.length === 0 ? "empty" : d.join(", ");
  return typeof d === "object" ? undefined : String(d);
}

function labelOf(key: string): string {
  const last = key.split(".").slice(-1)[0] ?? key;
  const text = last.replace(/_/g, " ");
  return text.charAt(0).toUpperCase() + text.slice(1);
}

/** Save needs a confirmation when the preview reports loosening, affected organizations or warnings. */
function needsConfirmation(p: SettingsPreview): boolean {
  return p.loosening.length > 0 || p.affected_orgs_total > 0 || p.warnings.length > 0;
}

/**
 * The generic section form (SET-1): renders any C-772 section — scalars, enums, booleans, whole lists,
 * keyed lists and write-only secrets — through `SettingField`. Save calls the preview route first and
 * asks for confirmation when something loosens; 409 / 422 follow `14 §Instance administration`.
 */
export function SettingsSectionForm({
  section,
  onReload,
}: {
  section: SettingsSection;
  /** Refetch the section (stale revision / locked key). */
  onReload: () => void;
}) {
  const initial = useMemo(() => initialDraft(section), [section]);
  const [draft, setDraft] = useState<Draft>(initial);
  const [error, setError] = useState<unknown>(null);
  const [saved, setSaved] = useState(false);
  const [confirm, setConfirm] = useState<SettingsPreview | null>(null);
  const preview = usePreviewSettings(section.section);
  const put = usePutSettings(section.section);
  const qc = useQueryClient();
  const busy = preview.isPending || put.isPending;

  const dirty = JSON.stringify(toValues(section, draft)) !== JSON.stringify(toValues(section, initial));
  const errs = error ? fieldErrors(error) : {};
  const knownKeys = new Set(section.settings.map((s) => s.key));
  const otherErrs = Object.entries(errs).filter(([k]) => !knownKeys.has(k.split("/")[0] ?? k));
  const code = error instanceof ApiError ? error.code : null;

  function setValue(key: string, v: unknown) {
    setSaved(false);
    setDraft((d) => ({ ...d, values: { ...d.values, [key]: v } }));
  }
  function resetValue(key: string) {
    setSaved(false);
    setDraft((d) => {
      const values = { ...d.values };
      delete values[key];
      return { ...d, values };
    });
  }
  function setSecret(key: string, sd: SecretDraft | null) {
    setSaved(false);
    setDraft((d) => {
      const secrets = { ...d.secrets };
      if (sd) secrets[key] = sd;
      else delete secrets[key];
      return { ...d, secrets };
    });
  }

  async function doPut() {
    setError(null);
    try {
      await put.mutateAsync({ generation: section.generation, values: toValues(section, draft) });
      setConfirm(null);
      setSaved(true);
    } catch (e) {
      setConfirm(null);
      setError(e);
      if (e instanceof ApiError && e.code === "setting_locked") {
        // Helm changed under the page: show what is locked now.
        void qc.invalidateQueries({ queryKey: ["admin", "settings"] });
        onReload();
      }
    }
  }

  async function submit(e?: FormEvent) {
    e?.preventDefault();
    setError(null);
    setSaved(false);
    try {
      const p = await preview.mutateAsync({ generation: section.generation, values: toValues(section, draft) });
      if (needsConfirmation(p)) setConfirm(p);
      else await doPut();
    } catch (err) {
      setError(err);
      if (err instanceof ApiError && err.code === "setting_locked") onReload();
    }
  }

  return (
    <form onSubmit={(e) => void submit(e)} noValidate className="max-w-3xl space-y-5">
      {saved ? (
        <Notice tone="accent" role="status">
          Saved. Applied to every replica within seconds.
        </Notice>
      ) : null}
      {code === "stale_revision" ? (
        <Notice tone="pending" role="alert" className="space-y-2">
          <p>Someone changed these settings meanwhile. Your edits are kept until you reload.</p>
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
        <Notice tone="danger" role="alert">
          {errorMessage(error)}
        </Notice>
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

      <FormSection title={section.title} description={section.description}>
        {section.settings.length === 0 ? (
          <p className="text-sm text-ink-2">No settings in this section yet.</p>
        ) : null}
        {section.settings.map((s) => (
          <SettingRow
            key={s.key}
            setting={s}
            draft={draft}
            error={errs[s.key]}
            entryErrors={Object.entries(errs).filter(([k]) => k.startsWith(`${s.key}/`))}
            setValue={setValue}
            resetValue={resetValue}
            setSecret={setSecret}
          />
        ))}
      </FormSection>

      <FormActions>
        <Button type="submit" disabled={busy || !dirty}>
          {busy ? "Saving…" : "Save"}
        </Button>
        <Button
          type="button"
          variant="outline"
          disabled={busy || !dirty}
          onClick={() => {
            setDraft(initial);
            setError(null);
            setSaved(false);
          }}
        >
          Discard changes
        </Button>
      </FormActions>

      <ConfirmDialog
        open={confirm !== null}
        onOpenChange={(o) => {
          if (!o) setConfirm(null);
        }}
        title="Save these settings?"
        description="This changes what people and agents are allowed to do."
        confirmLabel="Save"
        busy={put.isPending}
        onConfirm={() => void doPut()}
      >
        {confirm ? <ConfirmDetails preview={confirm} /> : null}
      </ConfirmDialog>
    </form>
  );
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
          <p className="text-ink-2">{preview.affected_orgs.map((o) => o.name).join(", ")}</p>
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
  draft,
  error,
  entryErrors,
  setValue,
  resetValue,
  setSecret,
}: {
  setting: Setting;
  draft: Draft;
  error?: string;
  entryErrors: [string, string][];
  setValue: (key: string, v: unknown) => void;
  resetValue: (key: string) => void;
  setSecret: (key: string, sd: SecretDraft | null) => void;
}) {
  const label = labelOf(s.key);
  const cid = settingControlId(s.key);

  if (s.secret) {
    const sd = draft.secrets[s.key];
    const fromEnv = s.secret.from_env;
    if (fromEnv) {
      return (
        <SettingField id={s.key} label={label} source="helm" helmKey={fromEnv} lockedValue="Set from the environment" />
      );
    }
    return (
      <SettingField id={s.key} label={label} source={sd ? "dashboard" : "default"} error={error}>
        <div id={cid} className="flex flex-wrap items-center gap-2">
          {sd?.mode === "replace" ? (
            <input
              aria-label={`${label}: new value`}
              type="password"
              autoComplete="off"
              className={`${inputClass} max-w-sm`}
              value={sd.value}
              onChange={(e) => setSecret(s.key, { mode: "replace", value: e.target.value })}
            />
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
  const kind = s.schema.kind;
  let control: ReactNode;
  if (kind === "bool") {
    control = (
      <input
        id={cid}
        type="checkbox"
        className="size-4 accent-accent"
        checked={current === true}
        onChange={(e) => setValue(s.key, e.target.checked)}
      />
    );
  } else if (kind === "enum") {
    control = (
      <select id={cid} className={inputClass} value={String(current ?? "")} onChange={(e) => setValue(s.key, e.target.value)}>
        {(s.schema.options ?? []).map((o) => (
          <option key={o} value={o}>
            {o}
          </option>
        ))}
      </select>
    );
  } else if (kind === "int" || kind === "duration_secs") {
    control = (
      <input
        id={cid}
        type="number"
        inputMode="numeric"
        className={`${inputClass} max-w-48`}
        min={s.schema.min ?? undefined}
        max={s.schema.max ?? undefined}
        value={current === null || current === undefined ? "" : String(current)}
        onChange={(e) => (e.target.value === "" ? resetValue(s.key) : setValue(s.key, Number(e.target.value)))}
      />
    );
  } else if (kind === "string") {
    control = (
      <input
        id={cid}
        className={inputClass}
        maxLength={s.schema.max_len ?? undefined}
        value={typeof current === "string" ? current : ""}
        onChange={(e) => setValue(s.key, e.target.value)}
      />
    );
  } else if (kind === "list" && s.schema.item_kind === "string") {
    control = (
      <textarea
        id={cid}
        rows={4}
        className={`${inputClass} font-mono`}
        placeholder="One per line"
        value={Array.isArray(current) ? (current as string[]).join("\n") : ""}
        onChange={(e) =>
          setValue(
            s.key,
            e.target.value
              .split("\n")
              .map((x) => x.trim())
              .filter(Boolean),
          )
        }
      />
    );
  } else {
    control = <JsonControl id={cid} value={current} onChange={(v) => setValue(s.key, v)} />;
  }
  return (
    <SettingField
      id={s.key}
      label={label}
      source={source}
      helmKey={s.helm_key}
      lockedValue={formatValue(s)}
      defaultLabel={defaultText(s)}
      onReset={() => resetValue(s.key)}
      error={error}
    >
      {control}
    </SettingField>
  );
}

/** A list or object edited as JSON text (the generic fallback; sections add typed editors). */
function JsonControl({ id, value, onChange }: { id: string; value: unknown; onChange: (v: unknown) => void }) {
  const [text, setText] = useState(JSON.stringify(value ?? null, null, 2));
  const [bad, setBad] = useState(false);
  return (
    <div>
      <textarea
        id={id}
        rows={5}
        spellCheck={false}
        aria-invalid={bad}
        className={`${inputClass} font-mono`}
        value={text}
        onChange={(e) => {
          setText(e.target.value);
          try {
            onChange(JSON.parse(e.target.value));
            setBad(false);
          } catch {
            setBad(true);
          }
        }}
      />
      {bad ? <p className="mt-1 text-xs text-danger-ink">Not valid JSON.</p> : null}
    </div>
  );
}

/** A keyed list: the Helm entries are locked ("Managed by Helm", read-only), the others are edited in a dialog. */
function KeyedList({
  setting: s,
  label,
  entries: mine,
  error,
  entryErrors,
  onChange,
}: {
  setting: Setting;
  label: string;
  entries: Record<string, unknown>[] | undefined;
  error?: string;
  entryErrors: [string, string][];
  onChange: (v: Record<string, unknown>[]) => void;
}) {
  const idField = s.id_field ?? "id";
  const own = mine ?? [];
  const helm = (s.entries ?? []).filter((e) => e.source === "helm");
  const [editing, setEditing] = useState<{ index: number | null; text: string } | null>(null);
  const [formError, setFormError] = useState<string | null>(null);
  const idError = entryErrors.find(([, m]) => m)?.[1];

  function save(e: FormEvent) {
    e.preventDefault();
    if (!editing) return;
    try {
      const v = JSON.parse(editing.text) as unknown;
      if (typeof v !== "object" || v === null || Array.isArray(v) || typeof (v as Record<string, unknown>)[idField] !== "string") {
        setFormError(`An entry is an object with a text \`${idField}\`.`);
        return;
      }
      const next = [...own];
      if (editing.index === null) next.push(v as Record<string, unknown>);
      else next[editing.index] = v as Record<string, unknown>;
      onChange(next);
      setEditing(null);
      setFormError(null);
    } catch {
      setFormError("Not valid JSON.");
    }
  }

  return (
    <SettingField id={s.key} label={label} source={own.length > 0 ? "dashboard" : "default"} error={error ?? idError}>
      <div id={settingControlId(s.key)} className="space-y-2">
        <ItemList aria-label={label}>
          {helm.map((e) => (
            <ListRow key={`h-${e.id}`}>
              <span className="font-medium text-ink">{e.id}</span>
              <Badge tone="pending">Managed by Helm</Badge>
            </ListRow>
          ))}
          {own.map((e, i) => (
            <ListRow
              key={`d-${String(e[idField])}-${i}`}
              actions={
                <>
                  <Button type="button" variant="ghost" size="sm" onClick={() => setEditing({ index: i, text: JSON.stringify(e, null, 2) })}>
                    Edit
                  </Button>
                  <Button type="button" variant="ghost" size="sm" onClick={() => onChange(own.filter((_, j) => j !== i))}>
                    Remove
                  </Button>
                </>
              }
            >
              <span className="font-medium text-ink">{String(e[idField])}</span>
              <Badge tone="accent">Dashboard</Badge>
            </ListRow>
          ))}
          {helm.length + own.length === 0 ? <ListRow>None yet.</ListRow> : null}
        </ItemList>
        <Button type="button" variant="outline" size="sm" onClick={() => setEditing({ index: null, text: JSON.stringify({ [idField]: "" }, null, 2) })}>
          Add entry…
        </Button>
      </div>
      <FormDialog
        open={editing !== null}
        onOpenChange={(o) => {
          if (!o) {
            setEditing(null);
            setFormError(null);
          }
        }}
        title={editing?.index === null ? "Add entry" : "Edit entry"}
        submitLabel="Done"
        onSubmit={save}
      >
        <textarea
          aria-label="Entry (JSON)"
          rows={10}
          spellCheck={false}
          className={`${inputClass} font-mono`}
          value={editing?.text ?? ""}
          onChange={(e) => setEditing((x) => (x ? { ...x, text: e.target.value } : x))}
        />
        {formError ? <p className="text-xs text-danger-ink">{formError}</p> : null}
      </FormDialog>
    </SettingField>
  );
}
