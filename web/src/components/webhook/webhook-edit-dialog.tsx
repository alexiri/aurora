"use client";

import { useState } from "react";

import { ErrorState, Loading } from "@/components/states";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent } from "@/components/ui/dialog";
import { Form, FormActions } from "@/components/ui/form";
import { ErrorNotice } from "@/components/ui/notice";
import { useWebhookPut, useWebhookView, type WebhookConfig, type WebhookView } from "@/lib/api/webhook";
import { configIssues, quickIssues, type Issue } from "@/lib/webhook/logic";

import {
  ExtrasStep,
  configForSave,
  extrasProblems,
  initialExtras,
  liveSecretNames,
  secretValuesForPut,
  secretsForPreset,
  signingForEdit,
  withoutBlankRows,
  type ExtrasState,
} from "./extras-step";
import { ReceiverStep } from "./receiver-step";
import { SigningSecretOnce } from "./signing-secret-once";

/** The editable configuration of a stored view: everything but what the server adds to a read. */
function configOf(v: WebhookView): WebhookConfig {
  return {
    version: 1,
    method: v.method,
    content_type: v.content_type,
    template: v.template,
    slots: v.slots,
    headers: v.headers,
    description: v.description,
    signing: v.signing,
    rate: v.rate,
    preset: v.preset,
  };
}

function EditForm({ view, connectionId, name, onClose }: { view: WebhookView; connectionId: string; name: string; onClose: () => void }) {
  const put = useWebhookPut(connectionId);
  const [step, setStep] = useState<0 | 1>(0);
  const [config, setConfig] = useState<WebhookConfig>(() => configOf(view));
  const [extras, setExtras] = useState<ExtrasState>(() => initialExtras({ secretNames: view.secret_value_names, signing: view.signing, rate: view.rate }));
  const [issues, setIssues] = useState<Issue[]>([]);
  const [attempted, setAttempted] = useState(false);
  const [error, setError] = useState<unknown>(null);
  const [shown, setShown] = useState<string | null>(null);
  // The ceiling is known to the browser only when it is what applies; a lower value of the connection's own hides it.
  const ceiling = view.effective_rate.source === "connection" ? null : view.effective_rate;
  const names = liveSecretNames(extras.secrets);
  const problems = attempted ? extrasProblems(extras, ceiling) : {};

  if (shown)
    return <SigningSecretOnce secret={shown} onDone={onClose} doneLabel="I have saved it, close" />;

  async function save() {
    setAttempted(true);
    const x = withoutBlankRows(extras);
    if (Object.keys(extrasProblems(x, ceiling)).length > 0) return;
    const local = config.template.trim() === "" ? [{ path: "template", code: "template_json", message: "Write the template, or choose a preset or an example." }] : quickIssues(config, liveSecretNames(x.secrets));
    if (local.length > 0) {
      setIssues(local);
      setStep(0);
      return;
    }
    const signingOn = x.signing === "keep" ? view.signing : x.signing !== "off";
    setError(null);
    setIssues([]);
    try {
      const res = await put.mutateAsync({ ...configForSave(config, x, signingOn), secret_values: secretValuesForPut(x), signing_secret: signingForEdit(x) });
      setExtras((cur) => ({ ...cur, signingValue: "", secrets: cur.secrets.map((r) => ({ ...r, value: "" })) }));
      if (typeof res.signing_secret === "string" && res.signing_secret) setShown(res.signing_secret);
      else onClose();
    } catch (err) {
      const cfg = configIssues(err);
      if (cfg.length > 0) {
        setIssues(cfg);
        setStep(cfg.some((i) => ["template", "slots", "content_type"].includes(i.path.split(".")[0] ?? "")) ? 0 : 1);
      } else setError(err);
    }
  }

  return (
    <Form
      aria-label={`Edit ${name}`}
      noValidate
      className="max-w-none space-y-5"
      onSubmit={(e) => {
        e.preventDefault();
        if (step === 0) {
          setStep(1);
        } else void save();
      }}
    >
      <p className="text-sm text-ink-2">
        Sends to <span className="font-medium text-ink">{view.host}</span>. The address itself is never shown. To change it, use Reconnect.
      </p>
      <p className="text-sm font-medium text-ink" aria-live="polite">
        {step === 0 ? "What the receiver expects" : "Extras"}
      </p>
      {step === 0 ? (
        <ReceiverStep
          config={config}
          onConfig={setConfig}
          secretNames={names}
          onPresetSecrets={(n) => setExtras((x) => ({ ...x, secrets: secretsForPreset(x.secrets, n) }))}
          issues={issues}
          onIssues={setIssues}
        />
      ) : (
        <ExtrasStep mode="edit" config={config} onConfig={setConfig} extras={extras} onExtras={setExtras} issues={issues} problems={problems} signingWas={view.signing} effective={view.effective_rate} ceiling={ceiling} />
      )}
      <ErrorNotice error={error} />
      <FormActions>
        {step === 1 ? (
          <Button type="button" variant="ghost" onClick={() => setStep(0)}>
            Back
          </Button>
        ) : (
          <Button type="button" variant="ghost" onClick={onClose}>
            Cancel
          </Button>
        )}
        {step === 0 ? (
          <Button type="submit" size="lg">
            Next
          </Button>
        ) : (
          <Button type="submit" size="lg" disabled={put.isPending}>
            {put.isPending ? "Saving…" : "Save changes"}
          </Button>
        )}
      </FormActions>
    </Form>
  );
}

/** Edit (C-913): steps 2 and 3 of the connect dialog on the stored configuration (`GET`/`PUT …/webhook`), with no address. */
export function WebhookEditDialog({ connectionId, name, open, onOpenChange }: { connectionId: string; name: string; open: boolean; onOpenChange: (o: boolean) => void }) {
  const q = useWebhookView(connectionId, open);
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent size="2xl" title={`Edit ${name}`}>
        {!open ? null : q.isError ? (
          <ErrorState error={q.error} onRetry={() => void q.refetch()} />
        ) : !q.data || !q.isFetchedAfterMount ? (
          <Loading />
        ) : (
          <EditForm view={q.data} connectionId={connectionId} name={name} onClose={() => onOpenChange(false)} />
        )}
      </DialogContent>
    </Dialog>
  );
}
