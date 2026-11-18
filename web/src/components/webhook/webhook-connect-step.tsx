"use client";

import { useQueryClient } from "@tanstack/react-query";
import { useState } from "react";

import type { SignInResult } from "@/components/apps/connect-journey/sign-in-step";
import { buildJourneyConnect } from "@/components/apps/connect-journey/logic";
import { ValueInput } from "@/components/credentials/value-input";
import { Button } from "@/components/ui/button";
import { Field, inputClass } from "@/components/ui/field";
import { Form, FormActions } from "@/components/ui/form";
import { Notice } from "@/components/ui/notice";
import { accountKeys, type Level } from "@/lib/api/accounts";
import type { AppDetail } from "@/lib/api/apps";
import { errorMessage } from "@/lib/api/errors";
import { connectWebhook, type WebhookConnectResult } from "@/lib/api/webhook";
import { configIssues, connectRefusal, emptyConfig, hostOfUrl, quickIssues, type Issue } from "@/lib/webhook/logic";

import { ExtrasStep, configForSave, extrasProblems, initialExtras, liveSecretNames, secretValuesForConnect, secretsForPreset, signingForConnect, withoutBlankRows, type ExtrasState } from "./extras-step";
import { ReceiverStep } from "./receiver-step";
import { SigningSecretOnce } from "./signing-secret-once";

const STEPS = ["Name and address", "What the receiver expects", "Extras"] as const;

/**
 * The connect steps of the Outgoing webhook app (C-913): the journey's Sign in step for this app, in three steps. No request
 * is sent to the receiver at connect (C-909). The address and the secret values are write-only: they leave this component's
 * state as soon as the request has been made, whatever it answered.
 */
export function WebhookSignInStep({
  app,
  workspaceId,
  orgId,
  orgMode,
  level,
  agentIds,
  hasWriteTools,
  onBack,
  onResult,
}: {
  app: AppDetail;
  workspaceId: string;
  orgId: string;
  orgMode?: boolean;
  level: Level | null;
  agentIds: string[];
  hasWriteTools: boolean;
  onBack?: () => void;
  onResult: (r: SignInResult) => void;
}) {
  const qc = useQueryClient();
  const [step, setStep] = useState<0 | 1 | 2>(0);
  const [name, setName] = useState("");
  const [url, setUrl] = useState("");
  const [config, setConfig] = useState(emptyConfig);
  const [extras, setExtras] = useState<ExtrasState>(() => initialExtras({ signing: false, rate: { per_minute: null, per_day: null } }));
  const [issues, setIssues] = useState<Issue[]>([]);
  const [fieldErrors, setFieldErrors] = useState<Record<string, string>>({});
  const [attempted, setAttempted] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<unknown>(null);
  const [shown, setShown] = useState<{ secret: string; result: SignInResult } | null>(null);
  const method = app.connect.find((m) => m.kind === "fields") ?? app.connect[0];

  const names = liveSecretNames(extras.secrets);
  const problems = attempted ? extrasProblems(extras, null) : {};

  function next() {
    setError(null);
    if (step === 0) {
      const errs: Record<string, string> = {};
      const n = name.trim();
      if (!n) errs.name = "Enter a name.";
      else if (n.length > 60) errs.name = "A name can be up to 60 characters.";
      if (!url.trim()) errs.url = "Paste the webhook address.";
      else if (!/^https:\/\//i.test(url.trim())) errs.url = "The address must start with https://.";
      else if (!hostOfUrl(url)) errs.url = "That is not a usable web address.";
      setFieldErrors(errs);
      if (Object.keys(errs).length === 0) setStep(1);
      return;
    }
    if (step === 1) {
      const local: Issue[] = config.template.trim() === "" ? [{ path: "template", code: "template_json", message: "Write the template, or choose a preset or an example." }] : quickIssues(config, names);
      setIssues(local);
      if (local.length === 0) setStep(2);
    }
  }

  async function submit() {
    if (!method) return;
    setAttempted(true);
    const x = withoutBlankRows(extras);
    if (Object.keys(extrasProblems(x, null)).length > 0) return;
    const body = configForSave(config, x, x.signing === "generate" || x.signing === "provided");
    setBusy(true);
    setError(null);
    setIssues([]);
    try {
      const base = buildJourneyConnect({
        workspaceId,
        method,
        fields: { name: name.trim(), url: url.trim() },
        agentIds,
        level,
        hasWriteTools,
        org: orgMode ? { orgId } : undefined,
        scopesOverride: null,
        scopes: null,
        accessLevel: method.access_levels[0]?.id,
      });
      const res: WebhookConnectResult = await connectWebhook({
        ...base,
        // The app's one method has exactly these two fields (C-909); the request never depends on the manifest's list.
        fields: { name: name.trim(), url: url.trim() },
        webhook: { ...body, secret_values: secretValuesForConnect(x), ...(signingForConnect(x) ? { signing_secret: signingForConnect(x)! } : {}) },
      });
      // Saved: the address and the secret values leave this component's state at once, and never come back.
      setUrl("");
      setExtras((cur) => ({ ...cur, signingValue: "", secrets: cur.secrets.map((r) => ({ ...r, value: "" })) }));
      void qc.invalidateQueries({ queryKey: ["apps"] });
      void qc.invalidateQueries({ queryKey: ["connections"] });
      void qc.invalidateQueries({ queryKey: accountKeys.all });
      const result: SignInResult = {
        accountId: res.connection.id,
        granted: (res.grants ?? []).map((g) => g.agent_id),
        grantFailed: null,
        missing: res.missing_scopes ?? [],
        scopes: res.connection.scopes,
        methodId: method.id,
      };
      if (res.signing_secret) setShown({ secret: res.signing_secret, result });
      else onResult(result);
    } catch (err) {
      const cfg = configIssues(err);
      const refusal = connectRefusal(err);
      if (cfg.length > 0) {
        setIssues(cfg);
        setStep(cfg.some((i) => ["template", "slots", "content_type"].includes(i.path.split(".")[0] ?? "")) ? 1 : 2);
      } else if (refusal?.field === "url") {
        setFieldErrors({ url: refusal.message });
        setStep(0);
      } else setError(refusal ? { code: "x", message: refusal.message } : err);
    } finally {
      setBusy(false);
    }
  }

  if (shown)
    return (
      <div className="space-y-3">
        <p className="text-base font-medium text-ink">{name.trim() || "The webhook"} is connected.</p>
        <SigningSecretOnce
          secret={shown.secret}
          onDone={() => {
            const r = shown.result;
            setShown(null);
            onResult(r);
          }}
        />
      </div>
    );

  const failure = error ? (typeof error === "object" && error !== null && "message" in error && !(error instanceof Error) ? String((error as { message: unknown }).message) : errorMessage(error)) : null;

  return (
    <Form
      aria-label={`Connect ${app.name}`}
      noValidate
      className="max-w-none space-y-5"
      onSubmit={(e) => {
        e.preventDefault();
        if (step < 2) next();
        else void submit();
      }}
    >
      <p className="text-sm font-medium text-ink" aria-live="polite">
        Step {step + 1} of 3: {STEPS[step]}
      </p>

      {step === 0 ? (
        <div className="space-y-4">
          <Field id="wh-name" label="Name" hint="So you can tell it apart, for example #ops-alerts." error={fieldErrors.name}>
            <input id="wh-name" className={inputClass} maxLength={60} autoComplete="off" value={name} onChange={(e) => setName(e.target.value)} />
          </Field>
          <ValueInput id="wh-url" label="Webhook address (URL)" value={url} onChange={setUrl} error={fieldErrors.url} />
          <p className="text-xs text-muted">
            Treat the address like a password: whoever has it can post. Aurora keeps it sealed, never shows it again and never gives it to an agent. Nothing is sent to it now.
          </p>
        </div>
      ) : null}

      {step === 1 ? (
        <ReceiverStep
          url={url}
          config={config}
          onConfig={setConfig}
          secretNames={names}
          onPresetSecrets={(n) => setExtras((x) => ({ ...x, secrets: secretsForPreset(x.secrets, n) }))}
          issues={issues}
          onIssues={setIssues}
        />
      ) : null}

      {step === 2 ? (
        <ExtrasStep mode="connect" config={config} onConfig={setConfig} extras={extras} onExtras={setExtras} issues={issues} problems={problems} ceiling={null} />
      ) : null}

      {failure ? (
        <Notice tone="danger" role="alert">
          {failure}
        </Notice>
      ) : null}
      <FormActions>
        {step > 0 ? (
          <Button type="button" variant="ghost" onClick={() => setStep((s) => (s - 1) as 0 | 1)}>
            Back
          </Button>
        ) : onBack ? (
          <Button type="button" variant="ghost" onClick={onBack}>
            Back
          </Button>
        ) : null}
        {step < 2 ? (
          <Button type="submit" size="lg">
            Next
          </Button>
        ) : (
          <Button type="submit" size="lg" disabled={busy}>
            {busy ? "Connecting…" : "Connect"}
          </Button>
        )}
      </FormActions>
    </Form>
  );
}
