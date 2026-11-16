"use client";

import { useState, type ChangeEvent, type FormEvent } from "react";

import { Button } from "@/components/ui/button";
import { Loading } from "@/components/states";
import { CopyBlock } from "@/components/account/ssh-copy";
import { ChoiceCards } from "@/components/ui/choice-cards";
import { Field, inputClass } from "@/components/ui/field";
import { Form, FormActions } from "@/components/ui/form";
import { Notice } from "@/components/ui/notice";
import { errorMessage } from "@/lib/api/errors";
import { useOrg } from "@/lib/api/queries";
import {
  buildLlmCreate,
  useCreateSecret,
  useLlmProviders,
  type Secret,
  type SecretScope,
} from "@/lib/api/secrets";

import { GiveAccessStep } from "./give-access";
import {
  NO_HARNESS,
  SHAPE_FIELDS,
  buildValue,
  harnessHint,
  keyReasonMessage,
  modeHelp,
  modeLabel,
  shapeOf,
  trustPolicy,
  valueComplete,
} from "./llm-key-modes";
import { OriginView } from "./origin-view";
import { ValueInput } from "./value-input";

/** The key file of a GCP service account: pasted, or read from a chosen file; never shown again. */
function KeyFileInput({ id, value, onChange }: { id: string; value: string; onChange: (v: string) => void }) {
  const [name, setName] = useState("");
  function pick(e: ChangeEvent<HTMLInputElement>) {
    const file = e.target.files?.[0];
    if (!file) return;
    setName(file.name);
    const reader = new FileReader();
    reader.onload = () => typeof reader.result === "string" && onChange(reader.result);
    reader.readAsText(file);
    // The file input keeps no copy: the text lives in the form state only until the key is saved.
    e.target.value = "";
  }
  return (
    <Field
      id={id}
      label="Service account key (JSON)"
      hint="Choose the downloaded .json file or paste its contents. The private key is never shown again."
    >
      <div className="space-y-2">
        <input
          type="file"
          accept="application/json,.json"
          aria-label="Choose a key file"
          className="block w-full max-w-full text-sm text-ink-2"
          onChange={pick}
        />
        {name ? <p className="break-all text-xs text-muted">Loaded {name}</p> : null}
        <textarea
          id={id}
          rows={4}
          spellCheck={false}
          autoComplete="off"
          className={`${inputClass} font-mono`}
          value={value}
          onChange={(e) => onChange(e.target.value)}
        />
      </div>
    </Field>
  );
}

export function LlmKeyForm({
  scope,
  orgId,
  preselect,
  onSaved,
}: {
  scope: SecretScope;
  orgId: string;
  /** Called once the key exists (the setup wizard refetches its status, which now counts this key). */
  onSaved?: (secret: Secret) => void;
  /** Agents ticked in the "Give agents access" step that follows (the getting-started guide: the workspace's own agent). */
  preselect?: string[];
}) {
  const providers = useLlmProviders(orgId);
  const org = useOrg(orgId);
  const create = useCreateSecret(scope);
  const [name, setName] = useState("");
  const [endpointId, setEndpointId] = useState("");
  const [authMode, setAuthMode] = useState("");
  const [value, setValue] = useState("");
  const [fields, setFields] = useState<Record<string, string>>({});
  const [clientErr, setClientErr] = useState("");
  const [saved, setSaved] = useState<Secret | null>(null);

  const endpoints = (providers.data?.endpoints ?? []).filter((e) => e.permitted === true);
  const endpoint = endpoints.find((e) => e.id === endpointId);
  const provider = providers.data?.providers.find((p) => p.id === endpoint?.provider);
  // The modes the endpoint admits (C-802), the recommended one first; an older answer falls back to the provider's.
  const admitted = endpoint?.auth_modes ?? provider?.auth_modes ?? [];
  const recommended = endpoint?.recommended_auth_mode ?? provider?.auth_mode_details?.find((d) => d.recommended)?.id ?? admitted[0] ?? "";
  const modes = [...admitted].sort((a, b) => Number(b === recommended) - Number(a === recommended));
  const mode = admitted.includes(authMode) ? authMode : recommended;
  const shape = shapeOf(provider?.auth_mode_details, mode);
  const scopes = endpoint?.key_scopes ?? [];
  const scopeName = scope.kind === "workspace" ? "workspace" : "organization";
  const scopeOk = !endpoint || scopes.length === 0 || scopes.includes(scopeName);
  const externalId = org.data?.external_id ?? "";

  async function submit(e: FormEvent) {
    e.preventDefault();
    if (!name.trim() || !endpointId) return setClientErr("Name and endpoint are required.");
    if (!valueComplete(shape, value, fields)) return setClientErr("Fill in every required field.");
    setClientErr("");
    setSaved(null);
    try {
      const s = await create.mutateAsync(
        buildLlmCreate({
          name,
          endpointId,
          authMode: admitted.length > 1 ? mode : "",
          value: buildValue(shape, value, fields),
        }),
      );
      setSaved(s);
      setName("");
      onSaved?.(s);
    } catch {
      // surfaced via create.error
    } finally {
      // Write-only (SEC-12): nothing typed stays in the form once it was sent.
      setValue("");
      setFields({});
    }
  }

  if (providers.isPending) return <Loading />;
  if (providers.isError)
    return <Notice tone="danger" role="alert">{errorMessage(providers.error)}</Notice>;
  if (endpoints.length === 0)
    return <Notice role="status">Your organization&apos;s governance does not permit any LLM endpoints.</Notice>;

  // The key exists: the form gives way to the "Give agents access" step (skippable, nothing preselected).
  if (saved)
    return (
      <GiveAccessStep
        subject={{ target: { kind: "secret", id: saved.id }, scope, orgId, preselect }}
        savedLabel={saved.name}
        onDone={() => setSaved(null)}
      >
        <OriginView origin={saved.origin} />
      </GiveAccessStep>
    );

  const harnessLine = endpoint ? harnessHint(endpoint.harnesses ?? []) : "";
  const noHarness = Boolean(endpoint) && endpoint?.harnesses !== undefined && endpoint.harnesses.length === 0;
  return (
    <div className="space-y-3">
      <Form onSubmit={(e) => void submit(e)} noValidate className="rounded-card border border-line bg-surface p-4 shadow-card sm:p-5" aria-label="New LLM key">
        <Field id="l-name" label="Name">
          <input id="l-name" className={inputClass} value={name} onChange={(e) => setName(e.target.value)} />
        </Field>
        <Field
          id="l-endpoint"
          label="Provider endpoint"
          hint={endpoint ? `Allowed key scopes: ${scopes.length ? scopes.join(", ") : "any"}.${harnessLine ? ` ${harnessLine}.` : ""}` : undefined}
          error={scopeOk ? undefined : `Governance does not permit ${scopeName}-level keys for this endpoint.`}
        >
          <select
            id="l-endpoint"
            className={inputClass}
            value={endpointId}
            onChange={(e) => {
              setEndpointId(e.target.value);
              setAuthMode("");
              setValue("");
              setFields({});
            }}
          >
            <option value="">Select an endpoint</option>
            {endpoints.map((ep) => {
              const h = harnessHint(ep.harnesses ?? []);
              return (
                <option key={ep.id} value={ep.id}>
                  {ep.display_name} ({ep.provider}){h ? ` — ${h}` : ""}
                </option>
              );
            })}
          </select>
        </Field>
        {noHarness ? (
          <Notice tone="pending" role="status">
            {NO_HARNESS}
          </Notice>
        ) : null}
        {modes.length > 1 ? (
          <ChoiceCards
            legend="How Aurora signs in to the provider"
            value={mode}
            onChange={(m) => {
              setAuthMode(m);
              setValue("");
              setFields({});
            }}
            options={modes.map((m) => ({
              value: m,
              label: modeLabel(m, endpoint?.provider),
              description: modeHelp(m, endpoint?.provider),
              badge: m === recommended ? "Recommended" : undefined,
            }))}
          />
        ) : null}
        {endpoint?.origin_method ? (
          <p className="text-xs text-muted">
            Your organization requires the key to belong to its own account; Aurora verifies this when you save
            (method: {endpoint.origin_method}).
          </p>
        ) : null}
        {endpoint && shape === "string" ? (
          <ValueInput id="l-value" label="API key" value={value} onChange={setValue} />
        ) : null}
        {endpoint && shape === "gcp_service_account" ? <KeyFileInput id="l-value" value={value} onChange={setValue} /> : null}
        {endpoint && shape !== "string" && shape !== "gcp_service_account"
          ? SHAPE_FIELDS[shape].map((f) =>
              f.secret ? (
                <ValueInput
                  key={`${mode}-${f.name}`}
                  id={`l-${f.name}`}
                  label={f.optional ? `${f.label} (optional)` : f.label}
                  value={fields[f.name] ?? ""}
                  onChange={(v) => setFields((x) => ({ ...x, [f.name]: v }))}
                />
              ) : (
                <Field key={`${mode}-${f.name}`} id={`l-${f.name}`} label={f.optional ? `${f.label} (optional)` : f.label} hint={f.hint}>
                  <input
                    id={`l-${f.name}`}
                    autoComplete="off"
                    spellCheck={false}
                    inputMode={f.numeric ? "numeric" : undefined}
                    className={inputClass}
                    value={fields[f.name] ?? ""}
                    onChange={(e) => setFields((x) => ({ ...x, [f.name]: e.target.value }))}
                  />
                </Field>
              ),
            )
          : null}
        {shape === "aws_assume_role" ? (
          <div role="group" className="space-y-2 rounded-control border border-line p-3" aria-label="Trust policy for the role">
            <p className="text-sm text-ink">
              Give the role this trust policy. It lets only your source user assume it, and only with this organization&apos;s
              external id.
            </p>
            {externalId ? (
              <>
                <p className="text-xs text-ink-2">
                  External id: <code className="break-all font-mono">{externalId}</code>
                </p>
                <CopyBlock text={trustPolicy(externalId)} label="the trust policy" />
              </>
            ) : (
              <p className="text-xs text-muted">Loading the organization&apos;s external id…</p>
            )}
          </div>
        ) : null}
        {shape === "aws_sigv4" ? (
          <p className="text-xs text-muted">Only the account id is shown after saving. Limit the key to bedrock:InvokeModel*.</p>
        ) : null}
        {endpoint && (endpoint.provider === "azure_openai" || endpoint.provider === "vertex") ? (
          <p className="text-xs text-muted">Models are configured by your administrator.</p>
        ) : null}
        {clientErr || create.error ? (
          <Notice tone="danger" role="alert">
            {clientErr || keyReasonMessage(create.error) || errorMessage(create.error)}
          </Notice>
        ) : null}
        <FormActions>
          <Button type="submit" size="lg" disabled={create.isPending || !scopeOk}>
            {create.isPending ? "Verifying…" : "Add LLM key"}
          </Button>
        </FormActions>
      </Form>
    </div>
  );
}
