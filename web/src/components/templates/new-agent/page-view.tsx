"use client";

import { useState } from "react";
import { useParams, useRouter } from "next/navigation";
import { useQueryClient } from "@tanstack/react-query";

import { useWorkspaceInternet } from "@/components/agents/agent-summary";
import { HANDLE_RE } from "@/components/agents/create-agent-dialog";
import { ConnectJourney } from "@/components/apps/connect-journey/connect-journey";
import { ErrorState, Loading } from "@/components/states";
import { ApiError } from "@/lib/api/client";
import {
  useCreateAgentFromTemplate,
  useTemplatePlan,
  workspaceTemplateKeys,
  type CreatedFromTemplate,
  type PlanChoice,
  type TemplatePlan,
} from "@/lib/api/agent-templates";
import { useOrg, useResolve, useWorkspace } from "@/lib/api/queries";
import { agentSummary } from "@/lib/summary/agent";
import { wsRoutes } from "@/lib/routes";
import { Button } from "@/components/ui/button";
import { Fold } from "@/components/ui/fold";
import { Notice } from "@/components/ui/notice";
import { PageHeader } from "@/components/ui/page-header";
import { SecretOnce } from "@/components/ui/secret-once";
import { Stepper } from "@/components/ui/stepper";
import { showUndoToast } from "@/components/ui/undo-toast";

import { TemplateReadme } from "../template-readme";
import { AboutStep } from "./about-step";
import { AccountsStep, resolveEntry, type AccountEntryState } from "./accounts-step";
import { AutomationsStep, automationEnabled, type AutomationState, type TemplateAutomation } from "./automations-step";
import { ReviewStep } from "./review-step";
import { asTemplateInputs } from "./words";

// P6-TPL-4 Part B1, C-1116: **Use this template** opens this page. It has the fold **About this template** (the
// README, closed) above four steps; each step asks its part of C-1111's create request and **Create** sends it. Each
// new webhook's secret is shown once, then the new agent's chat opens.

type StepId = "about" | "accounts" | "automations" | "review";

const STEPS: { id: StepId; label: string }[] = [
  { id: "about", label: "About" },
  { id: "accounts", label: "Accounts" },
  { id: "automations", label: "Automations" },
  { id: "review", label: "Review" },
];

type PlanAccount = PlanChoice["accounts"][number];

/** The summary's stand-in id for the agent that does not exist yet (C-986's sentences over the planned accounts). */
const PLANNED_AGENT_ID = "00000000-0000-0000-0000-000000000000";

/** One new webhook's secret, shown once after the create (C-1111: `secret` comes only in this response). */
interface WebhookSecret {
  name: string;
  secret: string;
}

/** The four-step page that creates an agent from a template (C-1116). */
export function NewAgentFromTemplateView() {
  const params = useParams<{ org: string; ws: string; template_id: string }>();
  const { org, ws } = params;
  const templateId = params.template_id;
  const routes = wsRoutes(org, ws);
  const router = useRouter();
  const qc = useQueryClient();
  const resolved = useResolve(org, ws);
  const wsId = resolved.data?.workspace?.id;
  const orgRecord = useOrg(resolved.data?.org.id);
  const workspace = useWorkspace(wsId);
  const plan = useTemplatePlan(wsId, templateId);
  const create = useCreateAgentFromTemplate(wsId ?? "");
  const internet = useWorkspaceInternet(wsId ?? "", resolved.data?.org.id ?? "");

  const [step, setStep] = useState<StepId>("about");
  const [name, setName] = useState("");
  const [handle, setHandle] = useState("");
  const [values, setValues] = useState<Record<string, string>>({});
  const [accounts, setAccounts] = useState<Record<number, AccountEntryState>>({});
  const [autos, setAutos] = useState<Record<string, AutomationState>>({});
  const [adds, setAdds] = useState<Record<number, boolean>>({});
  /** Client errors of the About step, and the mapped field errors of a refused create. */
  const [errors, setErrors] = useState<Record<string, string>>({});
  const [createError, setCreateError] = useState<ApiError | null>(null);
  /** A 409 `template_version_changed`: the plan was replaced; the person checks the changes and creates again. */
  const [updated, setUpdated] = useState(false);
  /** The new webhook secrets, shown one after another (each once), then the agent's chat opens. */
  const [secrets, setSecrets] = useState<WebhookSecret[] | null>(null);
  const [created, setCreated] = useState<CreatedFromTemplate | null>(null);
  /** The account-only connect journey the Accounts step opened: the entry and the app it is for. */
  const [connect, setConnect] = useState<{ index: number; appId: string } | null>(null);

  if (resolved.isPending || plan.isPending) return <Loading />;
  if (resolved.isError) return <ErrorState error={resolved.error} onRetry={() => void resolved.refetch()} />;
  if (!wsId) return <ErrorState error={{ code: "not_found" }} />;
  if (plan.isError) return <ErrorState error={plan.error} onRetry={() => void plan.refetch()} />;

  const template = plan.data;
  const inputs = asTemplateInputs(template.content.inputs);
  const automations = (template.content.automations as TemplateAutomation[] | null) ?? [];
  const agentName = name.trim() || "The agent";

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

  // The C-986 sentences over the accounts as chosen (the agent that does not exist yet) and the workspace's Internet
  // access; nothing about websites is said when the viewer cannot read them.
  const summary = (() => {
    const planned: PlanAccount[] = template.entries.flatMap((e) => {
      const r = resolveEntry(e, accounts[e.index]);
      if (!r.target || r.isNew || r.skipped || r.level == null) return [];
      return [
        {
          ...r.target,
          given: true,
          agents: [
            {
              agent: { id: PLANNED_AGENT_ID, name: agentName, handle: "" },
              level: r.level,
              workspace: { id: wsId, name: workspace.data?.name ?? "" },
            },
          ],
        } as PlanAccount,
      ];
    });
    return agentSummary({ agent: agentName, agentId: PLANNED_AGENT_ID, accounts: planned, internet: internet.access });
  })();

  const closeJourney = (index: number, appId: string) => {
    const before = new Set(
      (template.entries.find((e) => e.index === index)?.choices.find((c) => c.app_id === appId)?.accounts ?? []).map((a) => a.ref.id),
    );
    setConnect(null);
    void plan.refetch().then((fresh) => {
      // The just-connected account is the one the plan did not have; falling back to the first keeps the entry usable.
      const after = fresh.data?.entries.find((e) => e.index === index)?.choices.find((c) => c.app_id === appId)?.accounts ?? [];
      const made = after.map((a) => a.ref.id).find((id) => !before.has(id)) ?? after[0]?.ref.id;
      setAccounts((s) => (s[index]?.accountId === "new" ? { ...s, [index]: { ...s[index], accountId: made } } : s));
    });
  };

  const submit = async () => {
    const body = {
      template_id: templateId,
      version: template.version,
      // Create is hidden while the plan has no content hash, so this is always the hash
      // the reviewed version was stored with — never an empty string.
      content_sha256: template.content_sha256!,
      name: name.trim(),
      handle: handle.trim() || undefined,
      inputs: values,
      accounts: template.entries.flatMap((e) => {
        const r = resolveEntry(e, accounts[e.index]);
        if (!r.chosen || r.isNew || r.skipped || !r.accountId) return [];
        return [{ entry: e.index, app_id: r.chosen.app_id, connection_id: r.accountId, ...(r.levelChanged && r.level ? { level: r.level } : {}) }];
      }),
      skill_refs: template.skill_refs
        .filter((ref) => ref.kind === "library" && ref.available && (adds[ref.index] ?? true) && ref.library)
        .map((ref) => ({ index: ref.index, library_id: ref.library!.id, commit_sha: ref.library!.commit_sha })),
      automations: automations
        .filter((a) => a.kind === "schedule")
        .map((a) => ({ kind: "schedule", name: a.name, enabled: automationEnabled(a, autos) })),
    };
    setCreateError(null);
    setErrors({});
    try {
      const res = await create.mutateAsync(body);
      const made = res.webhooks.filter((w) => w.secret).map((w) => ({ name: w.webhook.name, secret: w.secret! }));
      if (made.length > 0) {
        setCreated(res);
        setSecrets(made);
      } else {
        finish(res);
      }
    } catch (err) {
      mapCreateError(err);
    }
  };

  const finish = (res: CreatedFromTemplate) => {
    setSecrets(null);
    setCreated(null);
    showUndoToast(`${res.agent.name} is ready`);
    router.push(routes.agent(res.agent.handle));
  };

  const mapCreateError = (err: unknown) => {
    if (err instanceof ApiError) {
      // The offered version moved on: swap the plan for the one the refusal carries and ask for a look.
      if (err.code === "template_version_changed") {
        const next = (err.details as { plan?: TemplatePlan } | null)?.plan;
        if (next) qc.setQueryData(workspaceTemplateKeys.plan(wsId, templateId), next);
        setUpdated(true);
        return;
      }
      // A 403 from Create is about the chosen account (the plan only lists the accounts the caller may
      // give, C-504): it belongs on the Accounts step like the other field errors, where another
      // account can be chosen. A template without account entries has nothing to choose there.
      if (err.code === "forbidden" && template.entries.length > 0) {
        setErrors({ accounts: err.message });
        setStep("accounts");
        return;
      }
      const fields = (err.details as { fields?: { path: string; message?: string }[] } | null)?.fields ?? [];
      const next: Record<string, string> = {};
      let at: StepId | null = null;
      for (const f of fields) {
        const words = f.message ?? err.message;
        if (f.path === "/name" || f.path === "/handle" || f.path.startsWith("/inputs/")) {
          const key = f.path.startsWith("/inputs/") ? `inputs.${f.path.slice("/inputs/".length)}` : f.path.slice(1);
          next[key] = words;
          at ??= "about";
        } else if (f.path.startsWith("/accounts")) {
          next.accounts = words;
          at ??= "accounts";
        } else if (f.path.startsWith("/automations")) {
          next.automations = words;
          at ??= "automations";
        } else if (f.path.startsWith("/skill_refs")) {
          next.skill_refs = words;
          at ??= "review";
        }
      }
      if (at) {
        setErrors(next);
        setStep(at);
        return;
      }
      setCreateError(err);
      return;
    }
    setCreateError(new ApiError("unknown", err instanceof Error ? err.message : "Something went wrong."));
  };

  const stepError = createError ? createError.message : step === "automations" ? errors.automations : undefined;

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
      {updated ? (
        <Notice tone="pending" role="status">
          <p>This template was updated. Check the changes, then create.</p>
        </Notice>
      ) : null}
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
      ) : step === "accounts" ? (
        <>
          {errors.accounts ? (
            <Notice tone="danger" role="alert">
              <p>{errors.accounts}</p>
            </Notice>
          ) : null}
          <AccountsStep
            entries={template.entries}
            state={accounts}
            onChange={(index, next) => setAccounts((s) => ({ ...s, [index]: next }))}
            onConnect={(index, appId) => setConnect({ index, appId })}
            orgName={orgRecord.data?.name}
          />
        </>
      ) : step === "automations" ? (
        <>
          {stepError ? (
            <Notice tone="danger" role="alert">
              <p>{stepError}</p>
            </Notice>
          ) : null}
          <AutomationsStep
            automations={automations}
            state={autos}
            onChange={(automation, next) => setAutos((s) => ({ ...s, [automation]: next }))}
            timezone={workspace.data?.timezone}
          />
        </>
      ) : (
        <>
          {createError ? (
            <Notice tone="danger" role="alert">
              <p>{createError.message}</p>
            </Notice>
          ) : null}
          {!template.content_sha256 ? (
            <Notice tone="pending" role="status">
              <p>The template&apos;s plan has no content hash — open the template again.</p>
            </Notice>
          ) : null}
          <ReviewStep plan={template} agentName={agentName} summary={summary} adds={adds} onAddRef={(index, add) => setAdds((s) => ({ ...s, [index]: add }))} errors={errors} />
        </>
      )}
      <div className="flex flex-wrap justify-end gap-2">
        {step !== "about" ? (
          <Button variant="outline" onClick={() => setStep(step === "review" ? "automations" : step === "automations" ? "accounts" : "about")}>
            Back
          </Button>
        ) : null}
        {step === "about" ? <Button onClick={aboutNext}>Next</Button> : null}
        {step === "accounts" || step === "automations" ? (
          <Button onClick={() => setStep(step === "accounts" ? "automations" : "review")}>Next</Button>
        ) : null}
        {step === "review" && template.content_sha256 ? (
          <Button onClick={() => void submit()} disabled={create.isPending}>
            {create.isPending ? "Creating…" : "Create"}
          </Button>
        ) : null}
      </div>
      {connect && wsId && resolved.data?.org.id ? (
        <ConnectJourney
          workspaceId={wsId}
          orgId={resolved.data.org.id}
          appId={connect.appId}
          accountOnly
          onClose={() => closeJourney(connect.index, connect.appId)}
        />
      ) : null}
      {secrets && secrets.length > 0 && secrets[0] ? (
        <SecretOnce
          secret={secrets[0].secret}
          title={`Webhook ${secrets[0].name}`}
          onClose={() => {
            const rest = secrets.slice(1);
            if (rest.length === 0) {
              if (created) finish(created);
            } else setSecrets(rest);
          }}
        />
      ) : null}
    </div>
  );
}
