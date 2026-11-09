"use client";

import { useState, type FormEvent } from "react";

import { Button } from "@/components/ui/button";
import { Loading } from "@/components/states";
import { Field, inputClass } from "@/components/ui/field";
import { Form, FormActions } from "@/components/ui/form";
import { Notice } from "@/components/ui/notice";
import { errorMessage } from "@/lib/api/errors";
import {
  buildLlmCreate,
  useCreateSecret,
  useLlmProviders,
  type Secret,
  type SecretScope,
} from "@/lib/api/secrets";

import { GiveAccessStep } from "./give-access";
import { OriginView } from "./origin-view";
import { ValueInput } from "./value-input";

export function LlmKeyForm({ scope, orgId }: { scope: SecretScope; orgId: string }) {
  const providers = useLlmProviders(orgId);
  const create = useCreateSecret(scope);
  const [name, setName] = useState("");
  const [endpointId, setEndpointId] = useState("");
  const [authMode, setAuthMode] = useState("");
  const [value, setValue] = useState("");
  const [clientErr, setClientErr] = useState("");
  const [saved, setSaved] = useState<Secret | null>(null);

  const endpoints = (providers.data?.endpoints ?? []).filter((e) => e.permitted === true);
  const endpoint = endpoints.find((e) => e.id === endpointId);
  const provider = providers.data?.providers.find((p) => p.id === endpoint?.provider);
  const modes = provider?.auth_modes ?? [];
  const scopes = endpoint?.key_scopes ?? [];
  const scopeName = scope.kind === "workspace" ? "workspace" : "organization";
  const scopeOk = !endpoint || scopes.length === 0 || scopes.includes(scopeName);

  async function submit(e: FormEvent) {
    e.preventDefault();
    if (!name.trim() || !endpointId || !value) return setClientErr("Name, endpoint and key are required.");
    setClientErr("");
    setSaved(null);
    try {
      const s = await create.mutateAsync(
        buildLlmCreate({ name, endpointId, authMode: modes.length > 1 ? authMode : "", value }),
      );
      setSaved(s);
      setName("");
    } catch {
      // surfaced via create.error
    } finally {
      setValue("");
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
        subject={{ target: { kind: "secret", id: saved.id }, scope, orgId }}
        savedLabel={saved.name}
        onDone={() => setSaved(null)}
      >
        <OriginView origin={saved.origin} />
      </GiveAccessStep>
    );

  return (
    <div className="space-y-3">
      <Form onSubmit={(e) => void submit(e)} noValidate className="rounded-card border border-line bg-surface p-4 shadow-card sm:p-5" aria-label="New LLM key">
        <Field id="l-name" label="Name">
          <input id="l-name" className={inputClass} value={name} onChange={(e) => setName(e.target.value)} />
        </Field>
        <Field
          id="l-endpoint"
          label="Provider endpoint"
          hint={endpoint ? `Allowed key scopes: ${scopes.length ? scopes.join(", ") : "any"}.` : undefined}
          error={scopeOk ? undefined : `Governance does not permit ${scopeName}-level keys for this endpoint.`}
        >
          <select
            id="l-endpoint"
            className={inputClass}
            value={endpointId}
            onChange={(e) => {
              setEndpointId(e.target.value);
              setAuthMode("");
            }}
          >
            <option value="">Select an endpoint</option>
            {endpoints.map((ep) => (
              <option key={ep.id} value={ep.id}>
                {ep.display_name} ({ep.provider})
              </option>
            ))}
          </select>
        </Field>
        {modes.length > 1 ? (
          <Field id="l-auth" label="Authentication mode">
            <select id="l-auth" className={inputClass} value={authMode} onChange={(e) => setAuthMode(e.target.value)}>
              <option value="">Default</option>
              {modes.map((m) => (
                <option key={m} value={m}>
                  {m}
                </option>
              ))}
            </select>
          </Field>
        ) : null}
        {endpoint?.origin_method ? (
          <p className="text-xs text-muted">
            Your organization requires the key to belong to its own account; Aurora verifies this when you save
            (method: {endpoint.origin_method}).
          </p>
        ) : null}
        <ValueInput id="l-value" label="API key" value={value} onChange={setValue} />
        {clientErr || create.error ? (
          <Notice tone="danger" role="alert">
            {clientErr || errorMessage(create.error)}
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
