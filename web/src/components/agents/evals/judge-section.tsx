"use client";

import { useState, type FormEvent } from "react";

import { ErrorState, Loading } from "@/components/states";
import { Badge } from "@/components/ui/badge";
import { Field, inputClass } from "@/components/ui/field";
import { CheckboxField } from "@/components/ui/form";
import { Notice } from "@/components/ui/notice";
import { SaveBar, useSaveBar } from "@/components/ui/save-bar";
import { Section } from "@/components/ui/section";
import { useModelReadiness } from "@/lib/api/credential-grants";
import { useEvalSettings, usePutEvalSettings, type EvalSettings } from "@/lib/api/evals";

import { evalErrorMessage, judgeTestMessage } from "./copy";

/** The judge control: who may change it, in words. Exported for the unit test of the disabled state. */
export const JUDGE_OWNER_ONLY = "Only workspace owners can choose the grader key. You can still turn the regression notice on or off.";

function JudgeForm({ agentId, settings, canOwn }: { agentId: string; settings: EvalSettings; canOwn: boolean }) {
  const put = usePutEvalSettings(agentId);
  // The granted keys are readable by owners only (`GET …/model-readiness` is `can_own`).
  const readiness = useModelReadiness(agentId, canOwn);
  const [secretId, setSecretId] = useState(settings.judge?.secret_id ?? "");
  const [modelId, setModelId] = useState(settings.judge?.model_id ?? "");
  const [notify, setNotify] = useState(settings.regression_notify);
  const bar = useSaveBar();
  const [tested, setTested] = useState<EvalSettings["test"]>(null);
  const testedText = judgeTestMessage(tested);
  const current = settings.judge;
  const keys = readiness.data?.keys ?? [];
  const known = keys.some((k) => k.secret_id === secretId);

  const changed = secretId !== (current?.secret_id ?? "") || modelId.trim() !== (current?.model_id ?? "") || notify !== settings.regression_notify;
  const incomplete = secretId !== "" && modelId.trim() === "";

  function submit(e: FormEvent) {
    e.preventDefault();
    bar.clear();
    if (incomplete) return;
    // A person who is not an owner sends the judge as it is: only the notice changes.
    const judge = canOwn
      ? secretId
        ? { secret_id: secretId, model_id: modelId.trim() }
        : null
      : current
        ? { secret_id: current.secret_id, model_id: current.model_id }
        : null;
    setTested(null);
    put.mutate(
      { judge, regression_notify: notify },
      {
        onSuccess: (saved) => {
          bar.flash();
          setTested(saved.test ?? null);
        },
      },
    );
  }

  return (
    <form onSubmit={submit} aria-label="Grader settings" className="space-y-4 rounded-card border border-line bg-surface p-4 shadow-card sm:p-5">
      {current ? (
        <p className="flex flex-wrap items-center gap-x-2 gap-y-1 text-sm text-ink-2">
          Current grader: <span className="font-medium text-ink">{current.secret_label}</span> with <code className="font-mono text-xs">{current.model_id}</code>
          {/* behaviour-4: a grader whose test just failed is not "Ready". */}
          {tested?.result === "failed" ? (
            <Badge tone="danger">Test failed</Badge>
          ) : (
            <Badge tone={current.usable ? "accent" : "danger"}>{current.usable ? "Ready" : "Not usable"}</Badge>
          )}
        </p>
      ) : (
        <p className="text-sm text-ink-2">No grader key is set. Number and text questions do not need one; a graded question cannot be scored without it.</p>
      )}
      <div className="grid gap-4 sm:grid-cols-2">
        <Field id="evj-key" label="Grader key" hint={canOwn ? "An AI key granted to this agent." : undefined}>
          <select id="evj-key" className={inputClass} value={secretId} disabled={!canOwn || put.isPending} onChange={(e) => setSecretId(e.target.value)}>
            <option value="">No grader key</option>
            {!known && current ? <option value={current.secret_id}>{current.secret_label}</option> : null}
            {keys.map((k) => (
              <option key={k.secret_id} value={k.secret_id}>
                {k.name} ({k.scope}){k.usable ? "" : ", not usable"}
              </option>
            ))}
          </select>
        </Field>
        <Field id="evj-model" label="Grader model" hint="The model the key calls to grade answers.">
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
      {testedText ? (
        <Notice tone={tested?.result === "ok" ? "accent" : "pending"} role="status" data-testid="judge-test-result">
          {testedText}
        </Notice>
      ) : null}
      {incomplete ? (
        <Notice tone="pending" role="status">
          Enter the grader model to save this key.
        </Notice>
      ) : null}
      <SaveBar
        dirty={changed}
        saving={put.isPending}
        saved={bar.saved}
        error={put.error ? evalErrorMessage(put.error) : undefined}
        onDiscard={() => {
          setSecretId(settings.judge?.secret_id ?? "");
          setModelId(settings.judge?.model_id ?? "");
          setNotify(settings.regression_notify);
          put.reset();
        }}
      />
    </form>
  );
}

export function JudgeSection({ agentId, canOwn }: { agentId: string; canOwn: boolean }) {
  const q = useEvalSettings(agentId);
  return (
    <Section id="eval-judge" title="Grader" description="Graded questions are read by a model you choose. It sees the question, your criteria and the agent's answer, never the agent's instructions.">
      {q.isPending ? <Loading /> : null}
      {q.isError ? <ErrorState error={q.error} onRetry={() => void q.refetch()} /> : null}
      {q.data ? <JudgeForm agentId={agentId} settings={q.data} canOwn={canOwn} /> : null}
    </Section>
  );
}
