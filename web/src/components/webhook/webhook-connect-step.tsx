"use client";

import { useQueryClient } from "@tanstack/react-query";
import { useState } from "react";

import type { SignInResult } from "@/components/apps/connect-journey/sign-in-step";
import { BackNext } from "@/components/apps/connect-journey/frame";
import { buildJourneyConnect } from "@/components/apps/connect-journey/logic";
import { ValueInput } from "@/components/credentials/value-input";
import { Field, inputClass } from "@/components/ui/field";
import { Form } from "@/components/ui/form";
import { Notice } from "@/components/ui/notice";
import { accountKeys, type Level } from "@/lib/api/accounts";
import type { AppDetail } from "@/lib/api/apps";
import { errorMessage } from "@/lib/api/errors";
import { connectWebhook, type WebhookConnectResult } from "@/lib/api/webhook";
import { configIssues, connectRefusal, emptyConfig, hostOfUrl, quickIssues, type Issue } from "@/lib/webhook/logic";

import { ExtrasStep, configForSave, extrasProblems, initialExtras, liveSecretNames, secretValuesForConnect, secretsForPreset, signingForConnect, withoutBlankRows, type ExtrasState } from "./extras-step";
import { ReceiverStep } from "./receiver-step";
import { SigningSecretOnce } from "./signing-secret-once";

export type WebhookStepId = "address" | "message";

/** What the connect request needs from the journey that hosts the steps. */
export interface WebhookConnectContext {
  app: AppDetail;
  workspaceId: string;
  orgId: string;
  orgMode?: boolean;
  level: Level | null;
  agentIds: string[];
  hasWriteTools: boolean;
  /** The refusal belongs to a step: the host shows it. */
  goTo: (step: WebhookStepId) => void;
  onResult: (r: SignInResult) => void;
}

/**
 * (P5-REFINE-13, access-4) The state of the Outgoing webhook's two connect steps, **Address** and **Message** (C-913). They are
 * steps of the journey itself, not a wizard inside it, so the state lives in the journey (`useWebhookDraft` is called there)
 * and the journey's footer carries Back and Next. No request is sent to the receiver at connect (C-909). The address and the
 * secret values are write-only: they leave this state as soon as the request has been made, whatever it answered.
 */
export function useWebhookDraft() {
  const qc = useQueryClient();
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

  const names = liveSecretNames(extras.secrets);

  /** The Address step's checks; the Message step opens only when they pass. */
  function checkAddress(): boolean {
    setError(null);
    const errs: Record<string, string> = {};
    const n = name.trim();
    if (!n) errs.name = "Enter a name.";
    else if (n.length > 60) errs.name = "A name can be up to 60 characters.";
    if (!url.trim()) errs.url = "Paste the address to send to.";
    else if (!/^https:\/\//i.test(url.trim())) errs.url = "The address must start with https://.";
    else if (!hostOfUrl(url)) errs.url = "That is not a usable web address.";
    setFieldErrors(errs);
    return Object.keys(errs).length === 0;
  }

  async function connect(ctx: WebhookConnectContext) {
    const method = ctx.app.connect.find((m) => m.kind === "fields") ?? ctx.app.connect[0];
    if (!method) return;
    setError(null);
    const local: Issue[] = config.template.trim() === "" ? [{ path: "template", code: "template_json", message: "Write the template, or choose a preset or an example." }] : quickIssues(config, names);
    setIssues(local);
    if (local.length > 0) return;
    setAttempted(true);
    const x = withoutBlankRows(extras);
    if (Object.keys(extrasProblems(x, null)).length > 0) return;
    const body = configForSave(config, x, x.signing === "generate" || x.signing === "provided");
    setBusy(true);
    try {
      const base = buildJourneyConnect({
        workspaceId: ctx.workspaceId,
        method,
        fields: { name: name.trim(), url: url.trim() },
        agentIds: ctx.agentIds,
        level: ctx.level,
        hasWriteTools: ctx.hasWriteTools,
        org: ctx.orgMode ? { orgId: ctx.orgId } : undefined,
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
      // Saved: the address and the secret values leave this state at once, and never come back.
      setUrl("");
      setExtras((cur) => ({ ...cur, signingValue: "", secrets: cur.secrets.map((r) => ({ ...r, value: "" })) }));
      void qc.invalidateQueries({ queryKey: ["apps"] });
      void qc.invalidateQueries({ queryKey: ["connections"] });
      void qc.invalidateQueries({ queryKey: accountKeys.all });
      const result: SignInResult = {
        accountId: res.connection.id,
        accountName: name.trim(),
        granted: (res.grants ?? []).map((g) => g.agent_id),
        grantFailed: null,
        missing: res.missing_scopes ?? [],
        scopes: res.connection.scopes,
        methodId: method.id,
      };
      if (res.signing_secret) setShown({ secret: res.signing_secret, result });
      else ctx.onResult(result);
    } catch (err) {
      const cfg = configIssues(err);
      const refusal = connectRefusal(err);
      if (cfg.length > 0) {
        setIssues(cfg);
        ctx.goTo("message");
      } else if (refusal?.field === "url") {
        setFieldErrors({ url: refusal.message });
        ctx.goTo("address");
      } else setError(refusal ? { code: "x", message: refusal.message } : err);
    } finally {
      setBusy(false);
    }
  }

  return { name, setName, url, setUrl, config, setConfig, extras, setExtras, issues, setIssues, fieldErrors, attempted, busy, error, shown, setShown, names, checkAddress, connect };
}

export type WebhookDraft = ReturnType<typeof useWebhookDraft>;

function failureText(error: unknown): string | null {
  if (!error) return null;
  return typeof error === "object" && error !== null && "message" in error && !(error instanceof Error) ? String((error as { message: unknown }).message) : errorMessage(error);
}

/** **Address**: where the agent's message goes, and what to call it. Back and Next are the journey's footer. */
export function WebhookAddressStep({ draft, agentName, onBack, onNext }: { draft: WebhookDraft; agentName?: string; onBack?: () => void; onNext: () => void }) {
  return (
    <>
      <Form
        id="wh-address-form"
        aria-label="Where the message goes"
        noValidate
        className="max-w-none space-y-4"
        onSubmit={(e) => {
          e.preventDefault();
          if (draft.checkAddress()) onNext();
        }}
      >
        <ValueInput
          id="wh-url"
          label="Address to send to"
          placeholder="https://hooks.example.com/ops-alerts"
          hint={`${agentName || "The agent"} sends a POST request here.`}
          value={draft.url}
          onChange={draft.setUrl}
          error={draft.fieldErrors.url}
        />
        <Field id="wh-name" label="Name" hint="So you can tell it apart, for example Ops alerts." error={draft.fieldErrors.name}>
          <input id="wh-name" className={inputClass} placeholder="Ops alerts" maxLength={60} autoComplete="off" value={draft.name} onChange={(e) => draft.setName(e.target.value)} />
        </Field>
        <p className="text-xs text-muted">
          Treat the address like a password: whoever has it can post. Aurora keeps it sealed, never shows it again and never gives it to an agent. Nothing is sent to it now.
        </p>
      </Form>
      <BackNext form="wh-address-form" onBack={onBack} nextLabel="Next" />
    </>
  );
}

/**
 * **Message**: what the message looks like (a preset, an example or the template and its slots), then the extras (secret
 * values, headers, signing, limits) in a fold that opens when something there is needed. Next here connects.
 */
export function WebhookMessageStep({ draft, ctx, onBack }: { draft: WebhookDraft; ctx: WebhookConnectContext; onBack?: () => void }) {
  const { shown } = draft;
  if (shown)
    return (
      <div className="space-y-3">
        <p className="text-base font-medium text-ink">{draft.name.trim() || "The webhook"} is connected.</p>
        <SigningSecretOnce
          secret={shown.secret}
          onDone={() => {
            const r = shown.result;
            draft.setShown(null);
            ctx.onResult(r);
          }}
        />
      </div>
    );
  const problems = draft.attempted ? extrasProblems(draft.extras, null) : {};
  const extrasIssue = draft.issues.some((i) => /^(headers|rate|signing|secret_values|method)/.test(i.path));
  const needsExtras = Object.keys(problems).length > 0 || draft.names.length > 0 || extrasIssue;
  const failure = failureText(draft.error);
  return (
    <>
      <Form
        id="wh-message-form"
        aria-label={`Connect ${ctx.app.name}`}
        noValidate
        className="max-w-none space-y-5"
        onSubmit={(e) => {
          e.preventDefault();
          void draft.connect(ctx);
        }}
      >
        <ReceiverStep
          url={draft.url}
          config={draft.config}
          onConfig={draft.setConfig}
          secretNames={draft.names}
          onPresetSecrets={(n) => draft.setExtras((x) => ({ ...x, secrets: secretsForPreset(x.secrets, n) }))}
          issues={draft.issues}
          onIssues={draft.setIssues}
        />
        <details className="rounded-control border border-line px-3 py-2" open={needsExtras || undefined}>
          <summary className="cursor-pointer text-sm font-medium text-ink">Secret values, headers, signing and limits</summary>
          <div className="mt-4">
            <ExtrasStep mode="connect" config={draft.config} onConfig={draft.setConfig} extras={draft.extras} onExtras={draft.setExtras} issues={draft.issues} problems={problems} ceiling={null} />
          </div>
        </details>
        <p role="status" aria-live="polite" className="text-sm text-ink-2">
          {draft.busy ? "Checking the address. Nothing is sent to it." : null}
        </p>
        {failure ? (
          <Notice tone="danger" role="alert">
            {failure}
          </Notice>
        ) : null}
      </Form>
      <BackNext form="wh-message-form" onBack={onBack} nextLabel="Connect" busy={draft.busy} busyLabel="Checking…" />
    </>
  );
}

/**
 * The organization's journey keeps one Sign in step for this app (its Done step is unchanged): the same two steps, switched
 * here, with Back and Next in the journey's footer as everywhere.
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
  const draft = useWebhookDraft();
  const [step, setStep] = useState<WebhookStepId>("address");
  const ctx: WebhookConnectContext = { app, workspaceId, orgId, orgMode, level, agentIds, hasWriteTools, goTo: setStep, onResult };
  return step === "address" ? (
    <WebhookAddressStep draft={draft} onBack={onBack} onNext={() => setStep("message")} />
  ) : (
    <WebhookMessageStep draft={draft} ctx={ctx} onBack={() => setStep("address")} />
  );
}
