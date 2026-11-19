"use client";

import { Button } from "@/components/ui/button";
import { Field, inputClass } from "@/components/ui/field";
import { SecretInput } from "@/components/ui/secret-input";
import { Notice } from "@/components/ui/notice";
import { Segmented } from "@/components/ui/segmented";
import { Select } from "@/components/ui/select";
import type { EffectiveRate, SigningEdit, WebhookConfig } from "@/lib/api/webhook";
import { firstMessage, LIMITS, parseRate, SOURCE_WORDS, type Issue } from "@/lib/webhook/logic";

// Step 3, the extras (C-913): the description for the agent, secret values, headers, signing, method and limits.

/** One secret value. `existing` ones came from the server and show only their name: Replace sets a value, Delete removes it. */
export interface SecretRow {
  key: string;
  name: string;
  value: string;
  existing: boolean;
  /** An existing value that is being replaced (its input is open). */
  replacing: boolean;
  removed: boolean;
}

export type SigningChoice = "off" | "keep" | "generate" | "provided";

export interface ExtrasState {
  secrets: SecretRow[];
  signing: SigningChoice;
  /** The pasted signing secret; never kept after a submit. */
  signingValue: string;
  rate: { per_minute: string; per_day: string };
}

export const SECRET_NAME = /^[a-z][a-z0-9_]{0,39}$/;

export function newSecretRow(name = ""): SecretRow {
  return { key: `s${Math.random().toString(36).slice(2, 9)}`, name, value: "", existing: false, replacing: false, removed: false };
}

export function initialExtras(opts: { secretNames?: string[]; signing: boolean; rate: WebhookConfig["rate"] }): ExtrasState {
  return {
    secrets: (opts.secretNames ?? []).map((n) => ({ ...newSecretRow(n), existing: true })),
    signing: opts.secretNames ? (opts.signing ? "keep" : "off") : "off",
    signingValue: "",
    rate: { per_minute: opts.rate.per_minute?.toString() ?? "", per_day: opts.rate.per_day?.toString() ?? "" },
  };
}

/** The secret rows a preset asks for: the rows already typed with a name in the preset keep their value; the rest are new. */
export function secretsForPreset(current: SecretRow[], names: string[]): SecretRow[] {
  const kept = current.filter((r) => r.existing);
  const fresh = names.filter((n) => !kept.some((k) => k.name === n)).map((n) => current.find((r) => !r.existing && r.name === n) ?? newSecretRow(n));
  return [...kept, ...fresh];
}

/** The names a configuration may use as `{{secret:name}}`: those that stay. */
export const liveSecretNames = (rows: SecretRow[]) => rows.filter((r) => !r.removed && r.name.trim() !== "").map((r) => r.name.trim());

/** Problems the browser can see in the extras, keyed by the editor's paths. The server's ceiling check stays authoritative. */
export function extrasProblems(x: ExtrasState, ceiling: { per_minute: number; per_day: number } | null): Record<string, string> {
  const out: Record<string, string> = {};
  const seen = new Set<string>();
  x.secrets.forEach((r, i) => {
    if (r.removed || blank(r)) return;
    const n = r.name.trim();
    if (!SECRET_NAME.test(n)) out[`secret_values.${i}.name`] = "Use a lowercase name of up to 40 letters, digits and underscores, starting with a letter.";
    else if (seen.has(n)) out[`secret_values.${i}.name`] = "Two secret values have this name.";
    if (!r.existing && r.value === "") out[`secret_values.${i}.value`] = "Enter the value.";
    else if (r.replacing && r.value === "") out[`secret_values.${i}.value`] = "Enter the new value, or cancel the replacement.";
    else if (r.value.length > LIMITS.secretValue) out[`secret_values.${i}.value`] = "A secret value can be up to 4 KiB.";
    seen.add(n);
  });
  if (x.signing === "provided" && x.signingValue === "") out.signing = "Paste the signing secret.";
  for (const k of ["per_minute", "per_day"] as const) {
    const v = parseRate(x.rate[k]);
    if (v === "invalid") out[`rate.${k}`] = "Enter a whole number of 1 or more, or leave empty.";
    else if (v !== null && ceiling && v > ceiling[k]) out[`rate.${k}`] = `This is above the limit of ${ceiling[k]} that applies here.`;
  }
  return out;
}

/** A new row left completely empty is nothing the person meant to add. */
const blank = (r: SecretRow) => !r.existing && r.name.trim() === "" && r.value === "";

/** Drop the empty rows before the checks and the request. */
export const withoutBlankRows = (x: ExtrasState): ExtrasState => ({ ...x, secrets: x.secrets.filter((r) => !blank(r)) });

/** The configuration to save: the limits from their fields, and whether signing ends up on. */
export function configForSave(config: WebhookConfig, x: ExtrasState, signingOn: boolean): WebhookConfig {
  const pm = parseRate(x.rate.per_minute);
  const pd = parseRate(x.rate.per_day);
  return { ...config, signing: signingOn, rate: { per_minute: pm === "invalid" ? null : pm, per_day: pd === "invalid" ? null : pd } };
}

/** `secret_values` of a connect request: every row has a value. */
export function secretValuesForConnect(x: ExtrasState): Record<string, string> {
  return Object.fromEntries(x.secrets.filter((r) => !r.removed && !blank(r)).map((r) => [r.name.trim(), r.value]));
}

/** `secret_values` of an edit: a value sets, `null` deletes, an absent name is kept (C-909). */
export function secretValuesForPut(x: ExtrasState): Record<string, string | null> {
  const out: Record<string, string | null> = {};
  for (const r of x.secrets) {
    if (blank(r)) continue;
    const name = r.name.trim();
    if (r.removed) out[name] = null;
    else if (!r.existing || (r.replacing && r.value !== "")) out[name] = r.value;
  }
  return out;
}

/** The request's `signing_secret` for a new connection (C-909). */
export function signingForConnect(x: ExtrasState): { mode: "generate" } | { mode: "provided"; value: string } | undefined {
  if (x.signing === "generate") return { mode: "generate" };
  if (x.signing === "provided") return { mode: "provided", value: x.signingValue };
  return undefined;
}

/** The request's `signing_secret` for an edit (C-909). */
export function signingForEdit(x: ExtrasState): SigningEdit {
  if (x.signing === "generate") return { mode: "generate" };
  if (x.signing === "provided") return { mode: "provided", value: x.signingValue };
  return { mode: x.signing === "off" ? "off" : "keep" };
}

export function ExtrasStep({
  mode,
  config,
  onConfig,
  extras,
  onExtras,
  issues,
  problems,
  signingWas,
  effective,
  ceiling,
}: {
  mode: "connect" | "edit";
  config: WebhookConfig;
  onConfig: (c: WebhookConfig) => void;
  extras: ExtrasState;
  onExtras: (x: ExtrasState) => void;
  issues: Issue[];
  /** From `extrasProblems`, shown once the person has tried to go on. */
  problems: Record<string, string>;
  /** Edit: whether signing is on now. */
  signingWas?: boolean;
  /** Edit: the limit in force now, with where it comes from. */
  effective?: EffectiveRate;
  /** The ceiling the browser can check against; null when it cannot be known here. */
  ceiling: { per_minute: number; per_day: number } | null;
}) {
  const setSecret = (i: number, patch: Partial<SecretRow>) => onExtras({ ...extras, secrets: extras.secrets.map((r, j) => (j === i ? { ...r, ...patch } : r)) });
  const live = extras.secrets.filter((r) => !r.removed).length;
  const setHeader = (i: number, patch: Partial<WebhookConfig["headers"][number]>) =>
    onConfig({ ...config, headers: config.headers.map((h, j) => (j === i ? { ...h, ...patch } : h)) });
  const err = (path: string) => problems[path] ?? firstMessage(issues, path);

  return (
    <div className="space-y-6">
      <Field
        id="wh-desc"
        label="Description for the agent (optional)"
        hint={`${config.description.length} of ${LIMITS.description}. For example: Posts to #ops-alerts. Use it only for production incidents; one message per incident.`}
        error={err("description")}
      >
        <textarea id="wh-desc" className={`${inputClass} min-h-20`} maxLength={LIMITS.description} value={config.description} onChange={(e) => onConfig({ ...config, description: e.target.value })} />
      </Field>

      <fieldset className="space-y-3">
        <legend className="font-display text-base font-semibold text-ink">Secret values</legend>
        <p className="text-sm text-ink-2">
          Values the receiver needs that the agent must never see, such as an API key. Use one in the template or a header as {"{{secret:name}}"}. Up to {LIMITS.secrets}.
          {mode === "edit" ? " Saved values are never shown again; replace or delete them." : ""}
        </p>
        {extras.secrets.map((r, i) =>
          r.removed ? (
            <div key={r.key} className="flex flex-wrap items-center gap-2 text-sm text-ink-2">
              <span>
                <span className="font-mono">{r.name}</span> will be deleted.
              </span>
              <Button type="button" size="sm" variant="ghost" onClick={() => setSecret(i, { removed: false })}>
                Keep it
              </Button>
            </div>
          ) : (
            <div key={r.key} className="space-y-2 rounded-control border border-line p-3">
              {r.existing ? (
                <p className="text-sm font-medium text-ink">
                  <span className="font-mono">{r.name}</span> <span className="font-normal text-muted">(saved)</span>
                </p>
              ) : (
                <Field id={`wh-sn-${i}`} label="Name" error={problems[`secret_values.${i}.name`] ?? firstMessage(issues, `secret_values.${r.name}`)}>
                  <input id={`wh-sn-${i}`} className={inputClass} spellCheck={false} autoComplete="off" value={r.name} onChange={(e) => setSecret(i, { name: e.target.value })} />
                </Field>
              )}
              {r.existing && firstMessage(issues, `secret_values.${r.name}`) ? (
                <p role="alert" className="text-xs text-danger-ink">
                  {firstMessage(issues, `secret_values.${r.name}`)}
                </p>
              ) : null}
              {!r.existing || r.replacing ? (
                <Field id={`wh-sv-${i}`} label={r.existing ? `New value of ${r.name}` : "Value"} error={problems[`secret_values.${i}.value`]}>
                  <SecretInput
                    id={`wh-sv-${i}`}
                    revealLabel={r.existing ? `new value of ${r.name}` : "value"}
                    autoComplete="new-password"
                    value={r.value}
                    onChange={(v) => setSecret(i, { value: v })}
                  />
                </Field>
              ) : null}
              <div className="flex flex-wrap gap-2">
                {r.existing && !r.replacing ? (
                  <Button type="button" size="sm" variant="outline" aria-label={`Replace ${r.name}`} onClick={() => setSecret(i, { replacing: true })}>
                    Replace
                  </Button>
                ) : null}
                {r.existing && r.replacing ? (
                  <Button type="button" size="sm" variant="ghost" onClick={() => setSecret(i, { replacing: false, value: "" })}>
                    Cancel replacing
                  </Button>
                ) : null}
                <Button
                  type="button"
                  size="sm"
                  variant="danger"
                  aria-label={`${r.existing ? "Delete" : "Remove"} ${r.name || "secret value"}`}
                  onClick={() => (r.existing ? setSecret(i, { removed: true, replacing: false, value: "" }) : onExtras({ ...extras, secrets: extras.secrets.filter((_, j) => j !== i) }))}
                >
                  {r.existing ? "Delete" : "Remove"}
                </Button>
              </div>
            </div>
          ),
        )}
        <Button type="button" size="sm" variant="outline" disabled={live >= LIMITS.secrets} onClick={() => onExtras({ ...extras, secrets: [...extras.secrets, newSecretRow()] })}>
          Add a secret value
        </Button>
      </fieldset>

      <fieldset className="space-y-3">
        <legend className="font-display text-base font-semibold text-ink">Headers</legend>
        <p className="text-sm text-ink-2">Extra headers the receiver needs, such as Authorization. A value may use {"{{secret:name}}"}. Up to {LIMITS.headers}.</p>
        {config.headers.map((h, i) => (
          <div key={i} className="space-y-2 rounded-control border border-line p-3">
            <Field id={`wh-hn-${i}`} label="Header name" error={firstMessage(issues, `headers.${i}.name`)}>
              <input id={`wh-hn-${i}`} className={inputClass} spellCheck={false} autoComplete="off" value={h.name} onChange={(e) => setHeader(i, { name: e.target.value })} />
            </Field>
            <Field id={`wh-hv-${i}`} label="Header value" error={firstMessage(issues, `headers.${i}.value`)}>
              <input id={`wh-hv-${i}`} className={inputClass} spellCheck={false} autoComplete="off" value={h.value} onChange={(e) => setHeader(i, { value: e.target.value })} />
            </Field>
            <Button type="button" size="sm" variant="danger" aria-label={`Remove header ${h.name || i + 1}`} onClick={() => onConfig({ ...config, headers: config.headers.filter((_, j) => j !== i) })}>
              Remove
            </Button>
          </div>
        ))}
        <Button type="button" size="sm" variant="outline" disabled={config.headers.length >= LIMITS.headers} onClick={() => onConfig({ ...config, headers: [...config.headers, { name: "", value: "" }] })}>
          Add a header
        </Button>
      </fieldset>

      <fieldset className="space-y-3">
        <legend className="font-display text-base font-semibold text-ink">Signing</legend>
        <p className="text-sm text-ink-2">
          With signing on, every request carries an <span className="font-mono">X-Aurora-Signature</span> header the receiver can check. The signing secret is shown once.
        </p>
        <Segmented
          label="Signing"
          value={extras.signing}
          onChange={(v) => onExtras({ ...extras, signing: v, signingValue: v === "provided" ? extras.signingValue : "" })}
          options={
            mode === "edit" && signingWas
              ? [
                  { value: "keep", label: "Keep" },
                  { value: "generate", label: "Make a new one" },
                  { value: "provided", label: "Paste one" },
                  { value: "off", label: "Off" },
                ]
              : [
                  { value: "off", label: "Off" },
                  { value: "generate", label: "Make one" },
                  { value: "provided", label: "Paste one" },
                ]
          }
        />
        {extras.signing === "provided" ? (
          <Field id="wh-signing" label="Signing secret" error={err("signing")}>
            <SecretInput
              id="wh-signing"
              revealLabel="signing secret"
              autoComplete="new-password"
              value={extras.signingValue}
              onChange={(v) => onExtras({ ...extras, signingValue: v })}
            />
          </Field>
        ) : null}
        {extras.signing === "generate" ? <p className="text-sm text-ink-2">Aurora makes the secret when you save, and shows it once.</p> : null}
      </fieldset>

      <Field id="wh-method" label="Method" error={firstMessage(issues, "method")}>
        <Select id="wh-method" value={config.method} onChange={(v) => onConfig({ ...config, method: v as "POST" | "PUT" })} options={[{ value: "POST", label: "POST" }, { value: "PUT", label: "PUT" }]} />
      </Field>

      <fieldset className="space-y-3">
        <legend className="font-display text-base font-semibold text-ink">Limits</legend>
        {effective ? (
          <p className="text-sm text-ink-2" data-testid="wh-effective">
            In force now: {effective.per_minute} a minute and {effective.per_day} a day, from {SOURCE_WORDS[effective.source]}.
          </p>
        ) : (
          <p className="text-sm text-ink-2">Leave these empty to use the limit your organization or administrator sets. A limit here can only be lower.</p>
        )}
        <Field id="wh-pm" label="Sends per minute (optional)" hint={ceiling ? `Up to ${ceiling.per_minute}.` : undefined} error={err("rate.per_minute")}>
          <input id="wh-pm" className={inputClass} inputMode="numeric" value={extras.rate.per_minute} onChange={(e) => onExtras({ ...extras, rate: { ...extras.rate, per_minute: e.target.value } })} />
        </Field>
        <Field id="wh-pd" label="Sends per day (optional)" hint={ceiling ? `Up to ${ceiling.per_day}.` : undefined} error={err("rate.per_day")}>
          <input id="wh-pd" className={inputClass} inputMode="numeric" value={extras.rate.per_day} onChange={(e) => onExtras({ ...extras, rate: { ...extras.rate, per_day: e.target.value } })} />
        </Field>
      </fieldset>

      {issues.some((i) => i.path === "signing") ? <Notice tone="danger">{firstMessage(issues, "signing")}</Notice> : null}
    </div>
  );
}
