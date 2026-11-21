"use client";

import { useId, useState, type ChangeEvent, type FormEvent, type ReactNode } from "react";
import { createPortal } from "react-dom";

import { Button } from "@/components/ui/button";
import { Loading } from "@/components/states";
import { CopyBlock } from "@/components/account/ssh-copy";
import { ChoiceCards } from "@/components/ui/choice-cards";
import { CopyValue } from "@/components/ui/copy-value";
import { Field, inputClass } from "@/components/ui/field";
import { Form, FormActions } from "@/components/ui/form";
import { ErrorNotice, Notice } from "@/components/ui/notice";
import { ApiError } from "@/lib/api/client";
import { serviceLine } from "@/lib/labels";
import { errorMessage } from "@/lib/api/errors";
import { preselectAgents } from "@/lib/accounts";
import { useAgents, useOrg } from "@/lib/api/queries";
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

/** A service's logo tile: its initials (a logo is not served for AI endpoints). */
function ServiceTile({ name }: { name: string }) {
  const initials = name.split(/\s+/).filter(Boolean).slice(0, 2).map((w) => w[0]!.toUpperCase()).join("");
  return (
    <span aria-hidden="true" className="flex size-12 shrink-0 items-center justify-center rounded-control bg-subtle text-sm font-semibold text-ink">
      {initials}
    </span>
  );
}

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

/** The dialog's footer row for `LlmKeyForm`: pass `footer` to `DialogContent` and `target` to the form. */
export function useFooterSlot(): { footer: ReactNode; target: HTMLElement | null } {
  const [target, setTarget] = useState<HTMLElement | null>(null);
  return { footer: <div ref={setTarget} className="flex flex-wrap justify-end gap-2" />, target };
}

export function LlmKeyForm({
  scope,
  orgId,
  preselect,
  onSaved,
  submitLabel = "Add AI key",
  onCancel,
  footerTarget,
}: {
  /**
   * Inside a dialog: the element of the dialog's footer row (`useFooterSlot`). The form then has no card of its own and
   * its Cancel and submit buttons render in that row. `null` while the row is not mounted; `undefined` outside a dialog.
   */
  footerTarget?: HTMLElement | null;
  /** The submit button's words ("Add key" in the Add an AI key dialog). */
  submitLabel?: string;
  /** Adds a Cancel button before the submit button (the dialog's). */
  onCancel?: () => void;
  scope: SecretScope;
  orgId: string;
  /** Called once the key exists (the setup wizard refetches its status, which now counts this key). */
  onSaved?: (secret: Secret) => void;
  /** Agents ticked in the "Give agents access" step that follows (the getting-started guide: the workspace's own agent). */
  preselect?: string[];
}) {
  const formId = useId();
  const providers = useLlmProviders(orgId);
  const org = useOrg(orgId);
  const create = useCreateSecret(scope);
  const [name, setName] = useState("");
  const [endpointId, setEndpointId] = useState("");
  const [authMode, setAuthMode] = useState("");
  const [value, setValue] = useState("");
  const [fields, setFields] = useState<Record<string, string>>({});
  const [clientErr, setClientErr] = useState("");
  const [rejected, setRejected] = useState(false);
  const [step, setStep] = useState<"service" | "key">("service");
  const [saved, setSaved] = useState<Secret | null>(null);
  // (C-988, C-925) A workspace's key is given to the ticked agents in the same request: the contextual agent, else the
  // only agent, else none. `picked` is what the person changed; until then the default applies.
  const wsId = scope.kind === "workspace" ? scope.id : undefined;
  const agents = useAgents(wsId);
  const agentList = agents.data?.items ?? [];
  const [picked, setPicked] = useState<Set<string> | null>(null);
  const defaults = preselect ?? preselectAgents(agentList, null);
  const ticked = picked ?? new Set(defaults);
  const toggleAgent = (id: string) => {
    const next = new Set(ticked);
    if (next.has(id)) next.delete(id);
    else next.add(id);
    setPicked(next);
  };

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
    if (step === "service") {
      if (endpointId && scopeOk) setStep("key");
      return;
    }
    if (!name.trim() || !endpointId) return setClientErr("Name and endpoint are required.");
    if (!valueComplete(shape, value, fields)) return setClientErr("Fill in every required field.");
    setClientErr("");
    setSaved(null);
    setRejected(false);
    try {
      const s = await create.mutateAsync(
        buildLlmCreate({
          name,
          endpointId,
          authMode: admitted.length > 1 ? mode : "",
          value: buildValue(shape, value, fields),
          agentIds: wsId ? agentList.filter((a) => ticked.has(a.id)).map((a) => a.id) : [],
        }),
      );
      setSaved(s);
      setName("");
      setPicked(null);
      // Write-only (SEC-12): once the key is saved, nothing typed stays in the form.
      setValue("");
      setFields({});
      onSaved?.(s);
    } catch (err) {
      // (P5-REFINE-13, access-8) A refused key stays in the field (masked), so a typo is fixed, not retyped. It was not saved.
      if (err instanceof ApiError && err.code === "llm_key_invalid") setRejected(true);
    }
  }

  if (providers.isPending) return <Loading />;
  if (providers.isError)
    return <ErrorNotice error={providers.error} />;
  if (endpoints.length === 0)
    return <Notice role="status">Your organization&apos;s governance does not permit any LLM endpoints.</Notice>;

  // A workspace's key was saved with its agents (C-988): one confirmation, no second step.
  if (saved && wsId)
    return (
      <section aria-label="New AI key saved" className="space-y-3 rounded-card border border-line bg-surface p-4 text-ink shadow-card sm:p-5">
        <p role="status" className="text-sm font-medium">
          Saved {saved.name}
        </p>
        <OriginView origin={saved.origin} />
        <Button variant="outline" size="sm" onClick={() => setSaved(null)}>
          Add another
        </Button>
      </section>
    );
  // The organization's key exists: the form gives way to the "Give agents access" step (skippable, nothing preselected).
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
      <Form
        id={formId}
        onSubmit={(e) => void submit(e)}
        noValidate
        className={footerTarget !== undefined ? "max-w-none" : "rounded-card border border-line bg-surface p-4 shadow-card sm:p-5"}
        aria-label="New AI key">
        {step === "service" ? (
          // (P5-REFINE-13, access-9) First the question, one card per service (a logo tile, the name and one plain line:
          // "Anthropic — Claude models. Works with Claude Code."); no provider id, no scopes. Then the key.
          <div className="space-y-2">
            <ChoiceCards<string>
              legend="Which service is this key for?"
              value={endpointId}
              onChange={(id) => {
                setEndpointId(id);
                setAuthMode("");
                setValue("");
                setFields({});
              }}
              options={endpoints.map((ep) => ({
                value: ep.id,
                label: ep.display_name,
                description: serviceLine(ep.provider, harnessHint(ep.harnesses ?? [])),
                icon: <ServiceTile name={ep.display_name} />,
              }))}
            />
            {!scopeOk ? <p role="alert" className="text-xs text-danger-ink">{`Your organization does not permit ${scopeName}-level keys for this endpoint.`}</p> : null}
          </div>
        ) : (
          <>
        <p className="text-sm text-ink-2">
          <span className="font-medium text-ink">{endpoint?.display_name}</span>
          {" · "}
          <button type="button" className="text-link underline" onClick={() => setStep("service")}>
            Change service
          </button>
        </p>
        <Field id="l-name" label="Name">
          <input id="l-name" className={inputClass} value={name} onChange={(e) => setName(e.target.value)} />
        </Field>
        {harnessLine ? <p className="text-xs text-muted">{harnessLine}.</p> : null}
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
          <ValueInput
            id="l-value"
            label="API key"
            value={value}
            onChange={setValue}
            error={rejected ? "The provider rejected this key. Check it and try again; what you typed is still here." : undefined}
          />
        ) : null}
        {endpoint && shape === "gcp_service_account" ? <KeyFileInput id="l-value" value={value} onChange={setValue} /> : null}
        {endpoint && shape === "aws_assume_role" && externalId ? (
          <CopyValue label="Your organization's external id" value={externalId} hint="Not a secret. Put it in the role's trust policy." />
        ) : null}
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
              <CopyBlock text={trustPolicy(externalId)} label="the trust policy" />
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
        {wsId ? (
          <fieldset className="space-y-1" aria-label="Who can use this key">
            <legend className="mb-1 text-sm font-semibold text-ink">Who can use this key</legend>
            {agents.isPending ? (
              <p className="text-sm text-muted">Loading agents…</p>
            ) : agentList.length === 0 ? (
              <p className="text-sm text-muted">No agents yet. Give the key to an agent from its Apps &amp; access tab.</p>
            ) : (
              agentList.map((a) => (
                <label key={a.id} className="flex min-h-9 items-center gap-2.5 text-sm">
                  <input
                    type="checkbox"
                    className="size-4 shrink-0 accent-accent"
                    checked={ticked.has(a.id)}
                    disabled={create.isPending}
                    onChange={() => toggleAgent(a.id)}
                    aria-label={`${a.name} can use this key`}
                  />
                  <span className="min-w-0 break-words font-medium text-ink">{a.name}</span>
                </label>
              ))
            )}
          </fieldset>
        ) : null}
          </>
        )}
        {clientErr || create.error ? (
          <Notice tone="danger" role="alert">
            {clientErr || keyReasonMessage(create.error) || errorMessage(create.error)}
          </Notice>
        ) : null}
        {footerTarget !== undefined ? (
          // (P5-POLISH-11, A1) In a dialog the buttons are the dialog's footer row, outside the scrolling body.
          footerTarget
            ? createPortal(
                <>
                  {onCancel ? (
                    <Button type="button" variant="ghost" onClick={onCancel}>
                      Cancel
                    </Button>
                  ) : null}
                  {step === "service" ? (
                    <Button type="button" className="ml-auto" disabled={!endpointId || !scopeOk} onClick={() => setStep("key")}>
                      Next
                    </Button>
                  ) : (
                    <Button type="submit" form={formId} className="ml-auto" disabled={create.isPending || !scopeOk}>
                      {create.isPending ? "Verifying…" : submitLabel}
                    </Button>
                  )}
                </>,
                footerTarget,
              )
            : null
        ) : (
          <FormActions>
            {onCancel ? (
              <Button type="button" variant="outline" size="lg" onClick={onCancel}>
                Cancel
              </Button>
            ) : null}
            {step === "service" ? (
              <Button type="button" size="lg" disabled={!endpointId || !scopeOk} onClick={() => setStep("key")}>
                Next
              </Button>
            ) : (
              <Button type="submit" size="lg" disabled={create.isPending || !scopeOk}>
                {create.isPending ? "Verifying…" : submitLabel}
              </Button>
            )}
          </FormActions>
        )}
      </Form>
    </div>
  );
}
