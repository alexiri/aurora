"use client";

import { useState, type FormEvent } from "react";

import { ErrorState, Loading } from "@/components/states";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Field, inputClass } from "@/components/ui/field";
import { CheckboxField, FormActions } from "@/components/ui/form";
import { Notice } from "@/components/ui/notice";
import { Section } from "@/components/ui/section";
import { useModelReadiness } from "@/lib/api/credential-grants";
import { useEvalSettings, usePutEvalSettings, type EvalSettings } from "@/lib/api/evals";

import { evalErrorMessage } from "./copy";

/** The judge control: who may change it, in words. Exported for the unit test of the disabled state. */
export const JUDGE_OWNER_ONLY = "Only workspace owners can choose the judge key. You can still turn the regression notice on or off.";

function JudgeForm({ agentId, settings, canOwn }: { agentId: string; settings: EvalSettings; canOwn: boolean }) {
  const put = usePutEvalSettings(agentId);
  // The granted keys are readable by owners only (`GET …/model-readiness` is `can_own`).
  const readiness = useModelReadiness(agentId, canOwn);
  const [secretId, setSecretId] = useState(settings.judge?.secret_id ?? "");
  const [modelId, setModelId] = useState(settings.judge?.model_id ?? "");
  const [notify, setNotify] = useState(settings.regression_notify);
  const [saved, setSaved] = useState(false);
  const current = settings.judge;
  const keys = readiness.data?.keys ?? [];
  const known = keys.some((k) => k.secret_id === secretId);

  const changed = secretId !== (current?.secret_id ?? "") || modelId.trim() !== (current?.model_id ?? "") || notify !== settings.regression_notify;
  const incomplete = secretId !== "" && modelId.trim() === "";

  function submit(e: FormEvent) {
    e.preventDefault();
    setSaved(false);
    if (incomplete) return;
    // A person who is not an owner sends the judge as it is: only the notice changes.
    const judge = canOwn
      ? secretId
        ? { secret_id: secretId, model_id: modelId.trim() }
        : null
      : current
        ? { secret_id: current.secret_id, model_id: current.model_id }
        : null;
    put.mutate({ judge, regression_notify: notify }, { onSuccess: () => setSaved(true) });
  }

  return (
    <form onSubmit={submit} aria-label="Judge settings" className="space-y-4 rounded-card border border-line bg-surface p-4 shadow-card sm:p-5">
      {current ? (
        <p className="flex flex-wrap items-center gap-x-2 gap-y-1 text-sm text-ink-2">
          Current judge: <span className="font-medium text-ink">{current.secret_label}</span> with <code className="font-mono text-xs">{current.model_id}</code>
          <Badge tone={current.usable ? "accent" : "danger"}>{current.usable ? "Ready" : "Not usable"}</Badge>
        </p>
      ) : (
        <p className="text-sm text-ink-2">No judge key is set. Number and text questions do not need one; a judged question cannot be scored without it.</p>
      )}
      <div className="grid gap-4 sm:grid-cols-2">
        <Field id="evj-key" label="Judge key" hint={canOwn ? "An AI key granted to this agent." : undefined}>
          <select id="evj-key" className={inputClass} value={secretId} disabled={!canOwn || put.isPending} onChange={(e) => setSecretId(e.target.value)}>
            <option value="">No judge key</option>
            {!known && current ? <option value={current.secret_id}>{current.secret_label}</option> : null}
            {keys.map((k) => (
              <option key={k.secret_id} value={k.secret_id}>
                {k.name} ({k.scope}){k.usable ? "" : ", not usable"}
              </option>
            ))}
          </select>
        </Field>
        <Field id="evj-model" label="Judge model" hint="The model the key calls to judge answers.">
          <input id="evj-model" className={`${inputClass} font-mono`} value={modelId} disabled={!canOwn || put.isPending || secretId === ""} onChange={(e) => setModelId(e.target.value)} />
        </Field>
      </div>
      {!canOwn ? (
        <Notice tone="neutral" role="status">
          {JUDGE_OWNER_ONLY}
        </Notice>
      ) : null}
      <CheckboxField
        id="evj-notify"
        label="Show a notice when a run scores lower"
        hint="When a run's pass rate is at least 10 points below the previous finished run with the same questions, this page says so. Nothing is sent anywhere else."
        checked={notify}
        onChange={setNotify}
      />
      {put.error ? (
        <Notice tone="danger" role="alert">
          {evalErrorMessage(put.error)}
        </Notice>
      ) : null}
      {incomplete ? (
        <Notice tone="pending" role="status">
          Enter the judge model to save this key.
        </Notice>
      ) : null}
      <FormActions className="gap-2">
        <Button type="submit" disabled={!changed || incomplete || put.isPending}>
          Save judge settings
        </Button>
        {saved && !changed ? (
          <span role="status" className="text-sm text-ink-2">
            Saved.
          </span>
        ) : null}
      </FormActions>
    </form>
  );
}

export function JudgeSection({ agentId, canOwn }: { agentId: string; canOwn: boolean }) {
  const q = useEvalSettings(agentId);
  return (
    <Section id="eval-judge" title="Judge" description="Judged questions are read by a model you choose. It sees the question, your criteria and the agent's answer, never the agent's instructions.">
      {q.isPending ? <Loading /> : null}
      {q.isError ? <ErrorState error={q.error} onRetry={() => void q.refetch()} /> : null}
      {q.data ? <JudgeForm agentId={agentId} settings={q.data} canOwn={canOwn} /> : null}
    </Section>
  );
}
