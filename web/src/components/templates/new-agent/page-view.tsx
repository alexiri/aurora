"use client";

import { useState } from "react";
import { useParams } from "next/navigation";

import { HANDLE_RE } from "@/components/agents/create-agent-dialog";
import { ConnectJourney } from "@/components/apps/connect-journey/connect-journey";
import { ErrorState, Loading } from "@/components/states";
import { Button } from "@/components/ui/button";
import { Fold } from "@/components/ui/fold";
import { PageHeader } from "@/components/ui/page-header";
import { Stepper } from "@/components/ui/stepper";
import { useTemplatePlan } from "@/lib/api/agent-templates";
import { useOrg, useResolve } from "@/lib/api/queries";
import { wsRoutes } from "@/lib/routes";

import { TemplateReadme } from "../template-readme";
import { AboutStep } from "./about-step";
import { AccountsStep, type AccountEntryState } from "./accounts-step";
import { asTemplateInputs } from "./words";

// P6-TPL-4 Part B1, C-1116: **Use this template** opens this page. It has the fold **About this template** (the
// README, closed) above its steps; each step asks its part of C-1111's create request and **Create** sends it.

type StepId = "about" | "accounts";

const STEPS: { id: StepId; label: string }[] = [
  { id: "about", label: "About" },
  { id: "accounts", label: "Accounts" },
];

/** The four-step page (About and Accounts so far; Automations and Review come with the rest of C-1116). */
export function NewAgentFromTemplateView() {
  const params = useParams<{ org: string; ws: string; template_id: string }>();
  const { org, ws } = params;
  const templateId = params.template_id;
  const routes = wsRoutes(org, ws);
  const resolved = useResolve(org, ws);
  const wsId = resolved.data?.workspace?.id;
  const orgRecord = useOrg(resolved.data?.org.id);
  const plan = useTemplatePlan(wsId, templateId);

  const [step, setStep] = useState<StepId>("about");
  const [name, setName] = useState("");
  const [handle, setHandle] = useState("");
  const [values, setValues] = useState<Record<string, string>>({});
  const [accounts, setAccounts] = useState<Record<number, AccountEntryState>>({});
  const [errors, setErrors] = useState<Record<string, string>>({});
  /** The account-only connect journey the Accounts step opened: the entry and the app it is for. */
  const [connect, setConnect] = useState<{ index: number; appId: string } | null>(null);

  if (resolved.isPending || plan.isPending) return <Loading />;
  if (resolved.isError) return <ErrorState error={resolved.error} onRetry={() => void resolved.refetch()} />;
  if (!wsId) return <ErrorState error={{ code: "not_found" }} />;
  if (plan.isError) return <ErrorState error={plan.error} onRetry={() => void plan.refetch()} />;

  const template = plan.data;
  const inputs = asTemplateInputs(template.content.inputs);

  const aboutNext = () => {
    const errs: Record<string, string> = {};
    const n = name.trim();
    if (!n || n.length > 80) errs.name = "Name must be 1 to 80 characters.";
    const h = handle.trim();
    if (h && !HANDLE_RE.test(h)) errs.handle = "Use lowercase letters, digits and dashes, starting with a letter (max 40).";
    for (const input of inputs) {
      if (input.required !== false && !(values[input.id] ?? input.default ?? "").trim()) errs[`inputs.${input.id}`] = "This is needed.";
    }
    setErrors(errs);
    if (Object.keys(errs).length === 0) setStep("accounts");
  };

  return (
    <div className="space-y-4" data-testid="new-agent-from-template">
      <PageHeader
        title={`New agent from ${template.name}`}
        description={template.summary}
        breadcrumb={[{ label: "Agents", href: routes.agents() }, { label: "New agent" }]}
        className="mb-2"
      />
      <Fold summary="About this template" data-testid="template-fold">
        <TemplateReadme source={template.readme} />
      </Fold>
      <Stepper steps={STEPS} current={step} label="New agent steps" />
      {step === "about" ? (
        <AboutStep
          name={name}
          onName={(v) => setName(v)}
          handle={handle}
          onHandle={(v) => setHandle(v)}
          inputs={inputs}
          values={values}
          onValue={(id, v) => setValues((s) => ({ ...s, [id]: v }))}
          errors={errors}
        />
      ) : (
        <AccountsStep
          entries={template.entries}
          state={accounts}
          onChange={(index, next) => setAccounts((s) => ({ ...s, [index]: next }))}
          onConnect={(index, appId) => setConnect({ index, appId })}
          orgName={orgRecord.data?.name}
        />
      )}
      <div className="flex flex-wrap justify-end gap-2">
        {step !== "about" ? (
          <Button variant="outline" onClick={() => setStep("about")}>
            Back
          </Button>
        ) : null}
        {step === "about" ? <Button onClick={aboutNext}>Next</Button> : null}
      </div>
      {connect && wsId && resolved.data?.org.id ? (
        <ConnectJourney
          workspaceId={wsId}
          orgId={resolved.data.org.id}
          appId={connect.appId}
          accountOnly
          onClose={() => {
            setConnect(null);
            // A just-connected account only shows up after a refetch of the plan.
            void plan.refetch();
          }}
        />
      ) : null}
    </div>
  );
}
