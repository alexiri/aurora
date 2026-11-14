"use client";

import { useState } from "react";

import { DynamicForm } from "@/components/apps/dynamic-form";
import { Markdown } from "@/components/apps/markdown";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent } from "@/components/ui/dialog";
import { Notice } from "@/components/ui/notice";
import { defaultValues, validateFields, visibleFields, type FieldValues } from "@/lib/api/apps";
import { useCreateIntegration, useProbeIntegration, type ChannelIntegration } from "@/lib/api/channels";
import { ApiError } from "@/lib/api/client";
import { formFields, type ConnectorInfo, type FormField, type SetupField } from "@/lib/api/connectors";

import { ConnectedDone } from "./connected-done";
import { MutationError, splitByStore } from "./shared";

type Step = "check" | "choose" | "done";
type Choices = Record<string, { value: string; label: string }[]>;

const STEPS: { id: Step; label: string }[] = [
  { id: "check", label: "Server and bot" },
  { id: "choose", label: "Team and options" },
  { id: "done", label: "Connected" },
];

/** Step 1 asks for what the probe needs (required fields that are not choices); step 2 the rest. */
const isChoice = (f: FormField) => (f as SetupField).choices === "from_validate";
const firstStep = (f: FormField) => f.required && !isChoice(f);

/**
 * (FIX-127, C-710/C-711) Connect a platform whose setup has a field that takes its options from
 * the platform: (1) the required fields, then **Check** runs the probe; (2) the choice field
 * becomes a select filled from the probe, plus the optional fields; **Connect** runs the create
 * (still the one authoritative validation); (3) the done screen with the next steps.
 */
export function IntegrationStepper({
  orgId,
  connector,
  onClose,
}: {
  orgId: string;
  connector: ConnectorInfo;
  onClose: () => void;
}) {
  const all = formFields(connector.integration_setup);
  const [values, setValues] = useState<FieldValues>(() => defaultValues(all));
  const [errors, setErrors] = useState<Record<string, string>>({});
  const [step, setStep] = useState<Step>("check");
  const [choices, setChoices] = useState<Choices>({});
  const [warnings, setWarnings] = useState<string[]>([]);
  const [noProbe, setNoProbe] = useState(false);
  const [created, setCreated] = useState<ChannelIntegration | null>(null);
  const probe = useProbeIntegration(orgId);
  const create = useCreateIntegration(orgId);

  const first = all.filter(firstStep);
  const second = all.filter((f) => !firstStep(f)).map((f) => {
    const opts = choices[f.name];
    // A choice field with a probe answer is a select; without one it stays free text.
    return isChoice(f) && opts?.length ? ({ ...f, type: "select", options: opts } as FormField) : f;
  });
  const fields = step === "check" ? first : second;
  const pending = probe.isPending || create.isPending;

  function payload(of: FormField[]) {
    const { config, credentials } = splitByStore(visibleFields(of, values), values);
    return { config: config as Record<string, never>, credentials: credentials as Record<string, never> };
  }

  function check(e: React.FormEvent) {
    e.preventDefault();
    const errs = validateFields(first, values);
    setErrors(errs);
    if (Object.keys(errs).length) return;
    probe.mutate(
      { connector_id: connector.id, ...payload(first) },
      {
        onSuccess: (r) => {
          setChoices(r.choices);
          setWarnings(r.warnings);
          setNoProbe(false);
          // One option: preselect it.
          for (const f of all.filter(isChoice)) {
            const opts = r.choices[f.name];
            if (opts?.length === 1) setValues((p) => ({ ...p, [f.name]: opts[0]!.value }));
          }
          setStep("choose");
        },
        onError: (err) => {
          // A connector without probe: type the field instead (create validates everything).
          if (err instanceof ApiError && err.code === "probe_unsupported") {
            setNoProbe(true);
            setStep("choose");
          }
        },
      },
    );
  }

  function connect(e: React.FormEvent) {
    e.preventDefault();
    const errs = validateFields(all, values);
    setErrors(errs);
    if (Object.keys(errs).length) return;
    create.mutate(
      { connector_id: connector.id, ...payload(all) },
      { onSuccess: (i) => { setCreated(i); setStep("done"); } },
    );
  }

  const hardProbeError = probe.error && !(probe.error instanceof ApiError && probe.error.code === "probe_unsupported");
  return (
    <Dialog open onOpenChange={(o) => !o && !pending && onClose()}>
      <DialogContent
        title={step === "done" ? `${connector.display_name} is connected` : `Connect ${connector.display_name}`}
        description={step === "done" ? "Three things to do next." : "Credentials are stored securely and are never shown again."}
      >
        <ol aria-label="Steps" className="mb-3 flex flex-wrap gap-x-4 gap-y-1 text-xs text-muted">
          {STEPS.map((s, i) => (
            <li key={s.id} aria-current={s.id === step ? "step" : undefined} className={s.id === step ? "font-semibold text-ink" : undefined}>
              {i + 1}. {s.label}
            </li>
          ))}
        </ol>
        {step === "done" && created ? (
          <ConnectedDone orgId={orgId} connector={connector} integration={created} values={values} choices={choices} onClose={onClose} />
        ) : (
          <form className="space-y-3" onSubmit={step === "check" ? check : connect} aria-busy={pending}>
            {step === "check" && connector.setup_guide_md ? (
              <details className="rounded-control border border-line bg-subtle p-3 text-sm text-ink-2">
                <summary className="cursor-pointer font-medium text-ink">Setup guide</summary>
                <div className="mt-2">
                  <Markdown source={connector.setup_guide_md} />
                </div>
              </details>
            ) : null}
            {step === "choose" && noProbe ? (
              <Notice tone="pending" role="status">
                This connector cannot list the choices. Type them in; connecting checks everything.
              </Notice>
            ) : null}
            {step === "choose"
              ? warnings.map((w) => (
                  <Notice key={w} tone="pending" role="status">
                    {w}
                  </Notice>
                ))
              : null}
            <DynamicForm
              fields={fields}
              values={values}
              errors={errors}
              onChange={(n, v) => setValues((p) => ({ ...p, [n]: v }))}
            />
            <MutationError error={step === "check" ? (hardProbeError ? probe.error : null) : create.error} />
            {pending ? (
              <p role="status" className="text-sm text-ink-2">
                {probe.isPending ? "Checking" : "Validating"} with {connector.display_name}… this can take up to 10 seconds.
              </p>
            ) : null}
            <div className="flex flex-wrap gap-2">
              {step === "choose" ? (
                <Button type="button" variant="outline" disabled={pending} onClick={() => setStep("check")}>
                  Back
                </Button>
              ) : null}
              <Button type="submit" size="lg" disabled={pending}>
                {step === "check" ? (probe.isPending ? "Checking…" : "Check") : create.isPending ? "Validating…" : "Connect"}
              </Button>
            </div>
          </form>
        )}
      </DialogContent>
    </Dialog>
  );
}
