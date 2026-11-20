"use client";

import { useState, type FormEvent } from "react";

import { formatCount } from "@/lib/format-date";

import { Button } from "@/components/ui/button";
import { FormDialog } from "@/components/ui/confirm-dialog";
import { Dialog, DialogClose, DialogContent } from "@/components/ui/dialog";
import { Field, inputClass } from "@/components/ui/field";
import { ErrorNotice, Notice } from "@/components/ui/notice";
import { Select } from "@/components/ui/select";
import { StatusDot } from "@/components/ui/status-dot";
import { useTestLlmEndpoint, type LlmEndpointTestResult } from "@/lib/api/settings";
import { MODEL_ID_PATTERN, ROLES, type Role } from "@/lib/model-roles";

import { CAPABILITIES, ModelDetailsFieldset, ModelRolesFieldset, type InfoRowDraft } from "./llm-model-fields";

export const PROVIDER_LABEL: Record<string, string> = {
  litellm: "LiteLLM",
  anthropic: "Anthropic",
  openai: "OpenAI",
  azure_openai: "Azure OpenAI",
  bedrock: "Amazon Bedrock",
  vertex: "Google Vertex AI",
  openai_compatible: "OpenAI-compatible server",
};

type Entry = Record<string, unknown>;

/** The fields each provider's endpoint carries beyond `id` and `display_name` (`15`, `08 §6`). */
interface ProviderFields {
  baseUrl: "required" | "optional" | "none";
  extra: ("organization" | "project" | "auth_header" | "models_path" | "region" | "project_id" | "location")[];
  models: "required" | "optional";
  defaultModel: "required" | "optional";
  allowedPaths: boolean;
  /** `key_origin.method` values the provider supports. */
  origin: string[];
}

const FIELDS: Record<string, ProviderFields> = {
  litellm: { baseUrl: "required", extra: [], models: "optional", defaultModel: "optional", allowedPaths: true, origin: ["endpoint", "litellm_team"] },
  anthropic: { baseUrl: "none", extra: [], models: "optional", defaultModel: "optional", allowedPaths: true, origin: ["provider_account"] },
  openai: { baseUrl: "none", extra: ["organization", "project"], models: "optional", defaultModel: "optional", allowedPaths: true, origin: ["provider_account"] },
  openai_compatible: { baseUrl: "required", extra: ["auth_header", "models_path"], models: "optional", defaultModel: "optional", allowedPaths: true, origin: ["endpoint"] },
  azure_openai: { baseUrl: "required", extra: [], models: "required", defaultModel: "required", allowedPaths: true, origin: ["endpoint", "provider_account"] },
  bedrock: { baseUrl: "optional", extra: ["region"], models: "optional", defaultModel: "optional", allowedPaths: false, origin: ["endpoint", "provider_account"] },
  vertex: { baseUrl: "none", extra: ["project_id", "location"], models: "required", defaultModel: "required", allowedPaths: false, origin: ["provider_account"] },
};

const EXTRA_LABEL: Record<string, { label: string; hint?: string }> = {
  organization: { label: "OpenAI organization", hint: "Optional. Sent as OpenAI-Organization." },
  project: { label: "OpenAI project", hint: "Optional. Sent as OpenAI-Project." },
  auth_header: { label: "Key header", hint: "Which header carries the key." },
  models_path: { label: "Models path", hint: "Default /v1/models" },
  region: { label: "Region", hint: "For example eu-central-1" },
  project_id: { label: "Project id", hint: "The Google Cloud project that serves the models." },
  location: { label: "Location", hint: "global, or a region such as europe-west1" },
};

const ORIGIN_LABEL: Record<string, string> = {
  endpoint: "A key that works on this endpoint is ours",
  litellm_team: "LiteLLM team",
  provider_account: "Provider account",
};

interface Draft {
  id: string;
  provider: string;
  display_name: string;
  base_url: string;
  default_model: string;
  models: string;
  allowed_models: string;
  allowed_paths: string;
  extra: Record<string, string>;
  method: string;
  org_ids: string;
  ws_ids: string;
  team_ids: string;
  /** (P5-MODELS-3, C-875) The model each role stands for on this endpoint; empty = unset. */
  roles: Record<Role, string>;
  /** (P5-MODELS-3, C-875) `model_info`, one row per model id; `k` is a stable React key. */
  info: InfoRowDraft[];
}

const CONTEXT_MIN = 1_000;
const CONTEXT_MAX = 10_000_000;
/** `MODEL_INFO_MAX` (C-875). */
const MODEL_INFO_MAX = 64;

const str =(v: unknown): string => (typeof v === "string" ? v : "");
const lines = (v: unknown): string => (Array.isArray(v) ? v.filter((x): x is string => typeof x === "string").join("\n") : "");
const split = (t: string): string[] =>
  t
    .split("\n")
    .map((x) => x.trim())
    .filter(Boolean);

function toDraft(e: Entry | null, provider: string): Draft {
  const o = (e?.key_origin ?? {}) as Record<string, unknown>;
  const extra: Record<string, string> = {};
  for (const k of Object.keys(EXTRA_LABEL)) extra[k] = str(e?.[k]);
  const roles = (e?.model_roles ?? {}) as Record<string, unknown>;
  const infoIn = (e?.model_info ?? {}) as Record<string, unknown>;
  return {
    id: str(e?.id),
    provider: str(e?.provider) || provider,
    display_name: str(e?.display_name),
    base_url: str(e?.base_url),
    default_model: str(e?.default_model),
    models: lines(e?.models),
    allowed_models: lines(e?.allowed_models),
    allowed_paths: lines(e?.allowed_paths),
    extra,
    method: str(o.method),
    org_ids: lines(o.allowed_organization_ids),
    ws_ids: lines(o.allowed_workspace_ids),
    team_ids: lines(o.allowed_team_ids),
    roles: {
      large: str(roles.large),
      medium: str(roles.medium),
      small: str(roles.small),
    },
    info: Object.entries(infoIn).map(([id, v], k) => {
      const i = (v ?? {}) as Record<string, unknown>;
      return {
        k,
        id,
        ctx: typeof i.context_window === "number" ? String(i.context_window) : "",
        caps: Array.isArray(i.capabilities) ? i.capabilities.filter((c): c is string => typeof c === "string") : [],
      };
    }),
  };
}

/** What is wrong with the roles and model details as typed, by field (the server stays authoritative). */
function modelErrors(d: Draft): { roles: Partial<Record<Role, string>>; info: Record<number, { id?: string; ctx?: string }>; any: boolean } {
  const roles: Partial<Record<Role, string>> = {};
  for (const r of ROLES) {
    const v = d.roles[r].trim();
    if (v && !MODEL_ID_PATTERN.test(v)) roles[r] = "Letters, digits and . _ : / @ - only, up to 200 characters.";
  }
  const info: Record<number, { id?: string; ctx?: string }> = {};
  const seen = new Set<string>();
  for (const row of d.info) {
    const id = row.id.trim();
    const e: { id?: string; ctx?: string } = {};
    if (!id) e.id = "Enter the model id.";
    else if (!MODEL_ID_PATTERN.test(id)) e.id = "Letters, digits and . _ : / @ - only, up to 200 characters.";
    else if (seen.has(id)) e.id = "This model is already listed.";
    seen.add(id);
    const c = row.ctx.trim();
    if (c && !(/^\d+$/.test(c) && Number(c) >= CONTEXT_MIN && Number(c) <= CONTEXT_MAX)) {
      e.ctx = `A whole number from ${formatCount(CONTEXT_MIN)} to ${formatCount(CONTEXT_MAX)}.`;
    }
    if (e.id || e.ctx) info[row.k] = e;
  }
  return { roles, info, any: Object.keys(roles).length > 0 || Object.keys(info).length > 0 || d.info.length > MODEL_INFO_MAX };
}

/** The entry the draft describes: the fields of the chosen provider, every key the dialog does not edit kept. */
function toEntry(d: Draft, base: Entry | null): Entry {
  const f = FIELDS[d.provider];
  const out: Entry = { ...(base ?? {}) };
  const put = (k: string, v: string | string[] | undefined) => {
    if (v === undefined || v === "" || (Array.isArray(v) && v.length === 0)) delete out[k];
    else out[k] = v;
  };
  out.id = d.id.trim();
  out.provider = d.provider;
  put("display_name", d.display_name.trim());
  put("base_url", f?.baseUrl === "none" ? undefined : d.base_url.trim());
  put("default_model", d.default_model.trim());
  put("models", split(d.models));
  put("allowed_models", split(d.allowed_models));
  put("allowed_paths", f?.allowedPaths === false ? undefined : split(d.allowed_paths));
  for (const k of Object.keys(EXTRA_LABEL)) put(k, f?.extra.includes(k as never) ? d.extra[k]?.trim() : undefined);
  const roles = Object.fromEntries(ROLES.filter((r) => d.roles[r].trim()).map((r) => [r, d.roles[r].trim()]));
  if (Object.keys(roles).length > 0) out.model_roles = roles;
  else delete out.model_roles;
  const info: Record<string, Entry> = {};
  for (const row of d.info) {
    const id = row.id.trim();
    if (!id) continue;
    const v: Entry = {};
    if (row.ctx.trim()) v.context_window = Number(row.ctx.trim());
    if (row.caps.length > 0) v.capabilities = CAPABILITIES.map((c) => c.id).filter((c) => row.caps.includes(c));
    info[id] = v;
  }
  if (Object.keys(info).length > 0) out.model_info = info;
  else delete out.model_info;
  if (d.method) {
    const o: Entry = { method: d.method };
    const [orgs, wss, teams] = [split(d.org_ids), split(d.ws_ids), split(d.team_ids)];
    if (orgs.length) o.allowed_organization_ids = orgs;
    if (wss.length) o.allowed_workspace_ids = wss;
    if (teams.length) o.allowed_team_ids = teams;
    out.key_origin = o;
  } else {
    delete out.key_origin;
  }
  return out;
}

const REASON: Record<string, string> = {
  destination_refused:
    "Aurora can't reach private addresses. Ask whoever installed Aurora to allow it.",
  dns: "The name could not be found from the API.",
  timeout: "It did not answer within 10 seconds.",
  connect: "The connection was refused.",
  tls: "The TLS certificate is not trusted.",
  unexpected_status: "It answered with a server error.",
  unreachable: "It could not be reached.",
};

function TestOutcome({ r }: { r: LlmEndpointTestResult }) {
  return (
    <div role="status" aria-label="Connection test result" className="space-y-1 rounded-control border border-line p-3">
      <StatusDot tone={r.reachable ? "accent" : "danger"}>
        {r.reachable
          ? `The endpoint answers${r.status ? ` (HTTP ${r.status})` : ""}.`
          : `The endpoint did not answer${r.error_code && REASON[r.error_code] ? `: ${REASON[r.error_code]}` : "."}`}
      </StatusDot>
      {r.reachable && r.status && r.status >= 400 ? (
        <p className="text-xs text-ink-2">An answer like this is expected without a key; the endpoint is there.</p>
      ) : null}
      {r.models && r.models.length > 0 ? (
        <p className="break-words text-xs text-ink-2">
          Models it listed: {r.models.slice(0, 12).join(", ")}
          {r.models.length > 12 ? ` and ${r.models.length - 12} more` : ""}
        </p>
      ) : null}
    </div>
  );
}

/** A Helm-defined endpoint: read-only, with where it is configured. */
export function HelmEndpointDialog({ entry, helmKey, onClose }: { entry: Entry; helmKey: string; onClose: () => void }) {
  const rows = Object.entries(entry).filter(([, v]) => v !== null && v !== undefined);
  return (
    <Dialog open onOpenChange={(o) => !o && onClose()}>
      <DialogContent title={`Endpoint ${str(entry.id)}`} description="Managed by Helm." size="xl">
        <div className="space-y-4">
          <Notice tone="pending" role="status">
            Configured in Helm (<code className="font-mono">{helmKey}</code>). Change it in the values file, or remove it
            there to hand it to the dashboard with its current definition.
          </Notice>
          <dl className="space-y-2 text-sm">
            {rows.map(([k, v]) => (
              <div key={k} className="flex flex-col gap-1 sm:flex-row sm:gap-3">
                <dt className="font-medium text-ink sm:w-40 sm:shrink-0">{k}</dt>
                <dd className="min-w-0 flex-1 break-words font-mono text-xs text-ink-2">
                  {typeof v === "string" ? v : JSON.stringify(v)}
                </dd>
              </div>
            ))}
          </dl>
          <div className="flex justify-end">
            <DialogClose asChild>
              <Button type="button" variant="outline">
                Close
              </Button>
            </DialogClose>
          </div>
        </div>
      </DialogContent>
    </Dialog>
  );
}

/** Add or edit a dashboard endpoint (SET-3): the provider's own fields, allowed models, key origin, paths, a connection test. */
export function EndpointDialog({
  existing,
  providers,
  helmIds,
  otherIds,
  errors,
  onSave,
  onClose,
}: {
  existing: Entry | null;
  /** The providers the endpoint may use: enabled, and offered by this release. */
  providers: string[];
  helmIds: string[];
  otherIds: string[];
  /** Field errors of a failed save for this entry, by field (`base_url`, `key_origin/method`, …). */
  errors: Record<string, string>;
  onSave: (e: Entry) => void;
  onClose: () => void;
}) {
  const [d, setD] = useState<Draft>(() => toDraft(existing, providers[0] ?? "litellm"));
  const [result, setResult] = useState<LlmEndpointTestResult | null>(null);
  const test = useTestLlmEndpoint();
  const f = FIELDS[d.provider] ?? FIELDS.litellm!;
  const set = <K extends keyof Draft>(k: K, v: Draft[K]) => {
    setResult(null);
    setD((x) => ({ ...x, [k]: v }));
  };
  const id = d.id.trim();
  const mErr = modelErrors(d);
  const offered = [...new Set([...split(d.models), ...split(d.allowed_models), d.default_model.trim()].filter(Boolean))];
  const idError =
    errors.id ??
    (id && helmIds.includes(id)
      ? `\`${id}\` is defined in Helm; choose another id`
      : id && otherIds.includes(id)
        ? `\`${id}\` is already an endpoint`
        : id && !/^[a-z0-9][a-z0-9_-]*$/.test(id)
          ? "Lowercase letters, digits, `_` or `-`."
          : undefined);
  const missing =
    !id ||
    (f.baseUrl === "required" && !d.base_url.trim()) ||
    (f.models === "required" && split(d.models).length === 0) ||
    (f.defaultModel === "required" && !d.default_model.trim()) ||
    mErr.any;
  const entry = () => toEntry(d, existing);
  function submit(e: FormEvent) {
    e.preventDefault();
    // The dialog sits inside the page's form in the React tree: its submit must not save the page.
    e.stopPropagation();
    if (idError || missing) return;
    onSave(entry());
  }
  const methods = f.origin;
  return (
    <FormDialog
      open
      size="xl"
      onOpenChange={(o) => !o && onClose()}
      title={existing ? `Edit ${str(existing.id)}` : "Add an AI endpoint"}
      description="AI keys are never part of an endpoint: organizations and workspaces add them as accounts."
      submitLabel="Done"
      submitDisabled={Boolean(idError) || missing}
      onSubmit={submit}
    >
      <div className="grid gap-3 sm:grid-cols-2">
        <Field id="ep-id" label="Endpoint id" error={idError} hint="Keys name the endpoint by this id. It cannot be changed later.">
          <input id="ep-id" required disabled={existing !== null} className={inputClass} value={d.id} onChange={(e) => set("id", e.target.value)} />
        </Field>
        <Field id="ep-provider" label="Provider" error={errors.provider}>
          <select
            id="ep-provider"
            className={inputClass}
            value={d.provider}
            disabled={existing !== null}
            onChange={(e) => {
              set("provider", e.target.value);
              set("method", "");
            }}
          >
            {providers.includes(d.provider) ? null : <option value={d.provider}>{PROVIDER_LABEL[d.provider] ?? d.provider}</option>}
            {providers.map((p) => (
              <option key={p} value={p}>
                {PROVIDER_LABEL[p] ?? p}
              </option>
            ))}
          </select>
        </Field>
      </div>
      <Field id="ep-name" label="Display name" hint="Shown in pickers. Optional." error={errors.display_name}>
        <input id="ep-name" className={inputClass} value={d.display_name} onChange={(e) => set("display_name", e.target.value)} />
      </Field>
      {f.baseUrl !== "none" ? (
        <Field
          id="ep-base"
          label="Base URL"
          error={errors.base_url}
          hint={
            d.provider === "azure_openai"
              ? "https://<resource>.openai.azure.com, host only"
              : d.provider === "bedrock"
                ? "Optional VPC endpoint, https and host only"
                : "https://llm.example.org (the address agents' requests go to)"
          }
        >
          <input id="ep-base" className={inputClass} value={d.base_url} onChange={(e) => set("base_url", e.target.value)} />
        </Field>
      ) : null}
      {f.extra.length > 0 ? (
        <div className="grid gap-3 sm:grid-cols-2">
          {f.extra.map((k) => (
            <Field key={k} id={`ep-${k}`} label={EXTRA_LABEL[k]!.label} hint={EXTRA_LABEL[k]!.hint} error={errors[k]}>
              {k === "auth_header" ? (
                <Select id={`ep-${k}`} value={d.extra[k] ?? ""} placeholder="authorization (default)" onChange={(v) => set("extra", { ...d.extra, [k]: v })} options={[{ value: "authorization", label: "authorization" }, { value: "api-key", label: "api-key" }, { value: "x-api-key", label: "x-api-key" }]} />
              ) : (
                <input id={`ep-${k}`} className={inputClass} value={d.extra[k] ?? ""} onChange={(e) => set("extra", { ...d.extra, [k]: e.target.value })} />
              )}
            </Field>
          ))}
        </div>
      ) : null}
      <div className="grid gap-3 sm:grid-cols-2">
        <Field id="ep-default" label="Default model" error={errors.default_model} hint={f.defaultModel === "required" ? "Required for this provider." : "What a hosted agent uses when it has no model of its own."}>
          <input id="ep-default" className={inputClass} value={d.default_model} onChange={(e) => set("default_model", e.target.value)} />
        </Field>
        <Field id="ep-models" label="Models" error={errors.models} hint={f.models === "required" ? "One per line. Required for this provider." : "Optional fallback list, one per line."}>
          <textarea id="ep-models" rows={3} spellCheck={false} className={`${inputClass} font-mono`} value={d.models} onChange={(e) => set("models", e.target.value)} />
        </Field>
      </div>
      <Field id="ep-allowed" label="Allowed models" error={errors.allowed_models} hint="One per line. Empty allows every model. Organizations can narrow this, never widen it.">
        <textarea id="ep-allowed" rows={3} spellCheck={false} className={`${inputClass} font-mono`} value={d.allowed_models} onChange={(e) => set("allowed_models", e.target.value)} />
      </Field>
      <ModelRolesFieldset
        provider={d.provider}
        roles={d.roles}
        onChange={(r) => set("roles", r)}
        offered={offered}
        errors={mErr.roles}
        serverError={errors.model_roles}
      />
      <ModelDetailsFieldset
        rows={d.info}
        onChange={(rows) => set("info", rows)}
        errors={mErr.info}
        serverError={errors.model_info}
        max={MODEL_INFO_MAX}
      />
      <fieldset className="space-y-3 rounded-control border border-line p-3">
        <legend className="px-1 text-sm font-medium text-ink">Key origin</legend>
        <Field id="ep-method" label="Method" error={errors["key_origin/method"] ?? errors.key_origin} hint="Which account a key must belong to before it may carry prompts.">
          <Select id="ep-method" value={d.method} placeholder="No rule" onChange={(v) => set("method", v)} options={methods.map((m) => ({ value: m, label: ORIGIN_LABEL[m] ?? m }))} />
        </Field>
        {d.method ? (
          <div className="grid gap-3 sm:grid-cols-3">
            <Field id="ep-orgs" label="Allowed organization ids" hint="One per line.">
              <textarea id="ep-orgs" rows={3} spellCheck={false} className={`${inputClass} font-mono`} value={d.org_ids} onChange={(e) => set("org_ids", e.target.value)} />
            </Field>
            <Field id="ep-wss" label="Allowed workspace ids" hint="One per line.">
              <textarea id="ep-wss" rows={3} spellCheck={false} className={`${inputClass} font-mono`} value={d.ws_ids} onChange={(e) => set("ws_ids", e.target.value)} />
            </Field>
            {d.method === "litellm_team" ? (
              <Field id="ep-teams" label="Allowed team ids" hint="One per line.">
                <textarea id="ep-teams" rows={3} spellCheck={false} className={`${inputClass} font-mono`} value={d.team_ids} onChange={(e) => set("team_ids", e.target.value)} />
              </Field>
            ) : null}
          </div>
        ) : null}
      </fieldset>
      {f.allowedPaths ? (
        <Field id="ep-paths" label="Allowed paths" error={errors.allowed_paths} hint="Extra paths the key may be used on, one per line (absolute; a trailing /* admits a subtree). Inference paths are always allowed.">
          <textarea id="ep-paths" rows={2} spellCheck={false} className={`${inputClass} font-mono`} value={d.allowed_paths} onChange={(e) => set("allowed_paths", e.target.value)} />
        </Field>
      ) : null}
      <div className="flex flex-wrap items-center gap-3">
        <Button
          type="button"
          variant="outline"
          disabled={test.isPending || missing || Boolean(idError)}
          onClick={() => {
            setResult(null);
            test.mutate(entry(), { onSuccess: setResult });
          }}
        >
          {test.isPending ? "Testing…" : "Test connection"}
        </Button>
        <span className="text-xs text-muted">One request to the models path with no credential. Nothing is saved.</span>
      </div>
      {test.isError ? (
        <ErrorNotice error={test.error} />
      ) : null}
      {result ? <TestOutcome r={result} /> : null}
    </FormDialog>
  );
}
