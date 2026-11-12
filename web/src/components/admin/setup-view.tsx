"use client";

import Link from "next/link";
import { useQueryClient } from "@tanstack/react-query";
import { useMemo, type ReactNode } from "react";

import { LlmKeyForm } from "@/components/credentials/llm-form";
import { ErrorState, Loading } from "@/components/states";
import { Badge } from "@/components/ui/badge";
import { buttonVariants } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { CheckboxField } from "@/components/ui/form";
import { Notice } from "@/components/ui/notice";
import { PageHeader } from "@/components/ui/page-header";
import { StatusDot } from "@/components/ui/status-dot";
import { errorMessage } from "@/lib/api/errors";
import { onboardingKeys, useAdminSetup, type SetupCheck, type SetupStatus, type SetupStep } from "@/lib/api/onboarding";
import { usePlacement, useSavePlacement, type PlacementView as Placement, type PutPlacement } from "@/lib/api/placement";
import { useSecrets } from "@/lib/api/secrets";
import { useOrgs, useWorkspaces } from "@/lib/api/queries";
import { landingPath } from "@/lib/nav/landing";
import { cn } from "@/lib/utils";

import { OwnerlessBanner } from "./ownerless-banner";
import { PlacementView } from "./placement-view";

type Tone = "accent" | "pending" | "neutral";

const STATE: Record<SetupCheck["status"], { label: string; tone: Tone }> = {
  ok: { label: "Ready", tone: "accent" },
  warning: { label: "Check this", tone: "pending" },
  todo: { label: "To do", tone: "neutral" },
};

const TITLES: Record<string, { title: string; lead: string }> = {
  roles: { title: "Sign-in and roles", lead: "Who is an admin, and how people get their roles." },
  placement: { title: "Where new users land", lead: "The organization and workspace a new person gets on day one." },
  llm: { title: "LLM access", lead: "Agents need an LLM key and a model to think." },
  runners: { title: "Agents can run", lead: "Agents run on their own computers, started by a runner, and reach the outside through the gateway." },
  optional: { title: "Optional", lead: "Nothing here is needed to start." },
};

/** What to do about a check that is not ready, in words. */
const FIX: Record<string, string> = {
  no_provider: "Add an OIDC provider so people can sign in. See docs/operator/sign-in.md.",
  no_role_mappings: "Map a role from your sign-in token to instance admin. See docs/operator/sign-in.md.",
  no_endpoints: "Add an LLM endpoint. See docs/operator/llm-endpoints.md.",
  endpoint_unreachable: "Check the endpoint's address and that the API can reach it.",
  no_default_model: "Set a default model so agents need not choose one by one.",
  no_runner: "Install a runner and register it. See docs/operator/runners.md.",
  runner_offline: "Check that the runner is running and can reach the API. See docs/operator/runners.md.",
  no_gateway: "Start the gateway. Without it agents cannot reach the internet or the LLM.",
  plugins_pending: "Review them under Plugins.",
};

function CheckRow({ c }: { c: SetupCheck }) {
  const s = STATE[c.status];
  const fix = c.status !== "ok" ? FIX[c.reason] : undefined;
  return (
    <li className="space-y-1 py-2 text-sm" data-reason={c.reason}>
      <div className="flex flex-wrap items-center gap-x-3 gap-y-1">
        <StatusDot tone={c.status === "ok" ? "accent" : c.status === "warning" ? "pending" : "neutral"}>{s.label}</StatusDot>
        <span className="min-w-0 break-words text-ink">{c.message}</span>
      </div>
      {c.value ? (
        <p className="break-all pl-0 font-mono text-xs text-ink-2">{c.value}</p>
      ) : null}
      {fix ? <p className="text-ink-2">{fix}</p> : null}
      {c.helm ? (
        <p className="text-xs text-muted">
          Helm value: <code className="font-mono text-ink-2">{c.helm}</code> (read-only here)
        </p>
      ) : null}
    </li>
  );
}

function Checks({ step }: { step: SetupStep }) {
  if (step.checks.length === 0) return null;
  return (
    <ul aria-label={`${TITLES[step.id]?.title ?? step.id} checks`} className="divide-y divide-line">
      {step.checks.map((c) => (
        <CheckRow key={c.id} c={c} />
      ))}
    </ul>
  );
}

function StepCard({ n, id, step, children }: { n: number; id: string; step?: SetupStep; children: ReactNode }) {
  const meta = TITLES[id] ?? { title: "Try it", lead: "Open your workspace and say hello." };
  const state = step ? STATE[step.status] : null;
  return (
    <li id={`setup-${id}`} className="scroll-mt-20">
      <Card role="region" aria-labelledby={`setup-${id}-h`} className="space-y-3">
        <div className="flex flex-wrap items-start gap-3">
          <span
            aria-hidden="true"
            className={cn(
              "inline-flex size-7 shrink-0 items-center justify-center rounded-full text-sm font-semibold",
              step?.status === "ok" ? "bg-accent text-on-accent" : "bg-subtle text-ink-2",
            )}
          >
            {n}
          </span>
          <div className="min-w-0 flex-1">
            <h2 id={`setup-${id}-h`} className="font-display text-base font-semibold text-ink">
              {meta.title}
            </h2>
            <p className="mt-0.5 text-sm text-ink-2">{meta.lead}</p>
          </div>
          {state ? <Badge tone={state.tone}>{state.label}</Badge> : null}
        </div>
        {children}
      </Card>
    </li>
  );
}

function RolesBody({ setup, step }: { setup: SetupStatus; step: SetupStep }) {
  const you = setup.you;
  return (
    <div className="space-y-3">
      {you ? (
        <div className="space-y-1 text-sm text-ink-2">
          <p>
            You signed in through <strong className="text-ink">{you.provider_name ?? you.provider}</strong>.
            {you.role_claim ? (
              <>
                {" "}
                Your token&apos;s <code className="font-mono text-ink">{you.role_claim}</code>:
              </>
            ) : null}
          </p>
          {you.values.length > 0 ? (
            <ul aria-label="Your role values" className="space-y-1">
              {you.values.map((v) => {
                const g = you.granted.find((x) => x.value === v);
                const what = g
                  ? [g.instance_admin ? "instance admin" : null, g.org ? `${g.org.role} of ${g.org.slug}` : null, ...g.workspaces.map((w) => `${w.role} of workspace ${w.slug}`)]
                      .filter(Boolean)
                      .join(", ")
                  : "";
                return (
                  <li key={v} className="flex flex-wrap items-center gap-2">
                    <code className="rounded-control bg-subtle px-1.5 py-0.5 font-mono text-xs text-ink">{v}</code>
                    <span>{what ? `→ ${what}` : "→ not mapped to anything"}</span>
                  </li>
                );
              })}
            </ul>
          ) : (
            <p>The token carried no role values.</p>
          )}
        </div>
      ) : (
        <p className="text-sm text-ink-2">Your last sign-in did not read a role claim, so there is nothing to show here.</p>
      )}
      <Checks step={step} />
      <Link href="/admin/identity" className={cn(buttonVariants({ variant: "outline", size: "sm" }))}>
        Open Identity
      </Link>
    </div>
  );
}

/** Build the whole-document PUT from the saved view with another starter-grant set (C-521). */
function putWithStarters(v: Placement, starters: { kind: "secret" | "connection"; id: string }[]): PutPlacement {
  return {
    mode: v.mode,
    default_org_id: v.default_org?.id,
    default_org_role: v.default_org_role,
    shared_workspaces: v.shared_workspaces.map((w) => ({ workspace_id: w.workspace_id, role: w.role })),
    personal_workspace: { enabled: v.personal_workspace.enabled, role: v.personal_workspace.role, name_template: v.personal_workspace.name_template },
    starter_grants: starters,
  };
}

/** "Give it to every new user's agent": the starter grant of the org key (`04 §New-user placement`). */
function StarterSwitch({ placement, secretId, onChanged }: { placement: Placement; secretId: string; onChanged: () => void }) {
  const save = useSavePlacement();
  const on = placement.starter_grants.some((g) => g.kind === "secret" && g.id === secretId);
  const usable = placement.saved && placement.mode === "shared_org" && placement.personal_workspace.enabled;
  function toggle(next: boolean) {
    const others = placement.starter_grants.filter((g) => !(g.kind === "secret" && g.id === secretId)).map((g) => ({ kind: g.kind, id: g.id }));
    save.mutate(putWithStarters(placement, next ? [...others, { kind: "secret", id: secretId }] : others), { onSuccess: onChanged });
  }
  return (
    <div className="space-y-2">
      <CheckboxField
        id="setup-starter"
        label="Give it to every new user's agent"
        hint="Each new person's agent can use this key from the first minute, so nobody has to add one."
        checked={on}
        disabled={!usable || save.isPending}
        onChange={toggle}
      />
      {!usable ? (
        <p className="text-sm text-ink-2">Save a shared organization with personal workspaces in the step above to turn this on.</p>
      ) : null}
      {save.error ? (
        <Notice tone="danger" role="alert">
          {errorMessage(save.error)}
        </Notice>
      ) : null}
    </div>
  );
}

function LlmBody({ step, onChanged }: { step: SetupStep; onChanged: () => void }) {
  const placement = usePlacement();
  const org = placement.data?.default_org ?? null;
  const secrets = useSecrets({ kind: "organization", id: org?.id ?? "" });
  const orgKey = (secrets.data?.items ?? []).find((s) => s.kind === "llm" && s.status === "active");
  const noDefaultModel = step.checks.some((c) => c.reason === "no_default_model");
  return (
    <div className="space-y-4">
      <Checks step={step} />
      {noDefaultModel ? (
        <Notice tone="pending" role="status">
          An endpoint has no default model. Agents on it need a model chosen one by one; until then their turns fail with
          &ldquo;no default model&rdquo;. Set <code className="font-mono">default_model</code> in the Helm values.
        </Notice>
      ) : null}
      {!org || !placement.data?.saved ? (
        <p className="text-sm text-ink-2">Choose the default organization in the step above, then add its LLM key here.</p>
      ) : (
        <div className="space-y-3">
          <h3 className="font-display text-sm font-semibold text-ink">{orgKey ? `LLM key of ${org.name}` : `Add an organization LLM key for ${org.name}`}</h3>
          {orgKey ? (
            <p className="text-sm text-ink-2">
              <strong className="text-ink">{orgKey.name}</strong> belongs to {org.name}. It is stored sealed and never shown again.
            </p>
          ) : (
            <LlmKeyForm scope={{ kind: "organization", id: org.id }} orgId={org.id} />
          )}
          {orgKey ? <StarterSwitch placement={placement.data} secretId={orgKey.id} onChanged={onChanged} /> : null}
        </div>
      )}
    </div>
  );
}

function TryBody() {
  const placement = usePlacement();
  const orgs = useOrgs();
  // The step promises "if Daneel answers, everything above works": that is the placement's default
  // organization — the one the wizard set up with its key — not whichever org happens to be first
  // in the admin's list (their own personal organization predates the placement and has no key).
  const org = placement.data?.default_org ?? orgs.data?.items[0];
  const workspaces = useWorkspaces(org?.id);
  const href = useMemo(() => (org ? (landingPath([org], workspaces.data?.items) ?? "/") : "/"), [org, workspaces.data]);
  return (
    <div className="space-y-2 text-sm text-ink-2">
      <p>Open your workspace and say hello to Daneel. If Daneel answers, everything above works.</p>
      {/* `text-on-primary!`: the global `a { color: inherit }` would otherwise win over the utility. */}
      <Link href={href} className={cn(buttonVariants({ size: "sm" }), "text-on-primary!")}>
        Open your workspace
      </Link>
    </div>
  );
}

/** Admin › Setup (`14 §Onboarding` 1): the instance setup wizard. One page, a stepper, live status per step. */
export function SetupView() {
  const qc = useQueryClient();
  const setup = useAdminSetup();
  const refresh = () => void qc.invalidateQueries({ queryKey: onboardingKeys.setup });
  const step = (id: string) => setup.data?.steps.find((s) => s.id === id);

  return (
    <div className="max-w-[1160px] space-y-6">
      <PageHeader
        title="Set up Aurora"
        description="Six steps from a fresh install to your first answer. Each shows what is ready now."
        className="mb-0"
      />
      {setup.isPending ? <Loading /> : null}
      {setup.isError ? <ErrorState error={setup.error} onRetry={() => setup.refetch()} /> : null}
      <OwnerlessBanner />
      {setup.data ? (
        <>
          {!setup.data.placement_saved ? (
            <Notice tone="pending" role="status">
              This page opens at sign-in until you save where new users land (step 2).
            </Notice>
          ) : null}
          <ol aria-label="Setup steps" className="space-y-4">
            <StepCard n={1} id="roles" step={step("roles")}>
              {step("roles") ? <RolesBody setup={setup.data} step={step("roles")!} /> : null}
            </StepCard>
            <StepCard n={2} id="placement" step={step("placement")}>
              <PlacementView embedded onSaved={refresh} />
            </StepCard>
            <StepCard n={3} id="llm" step={step("llm")}>
              {step("llm") ? <LlmBody step={step("llm")!} onChanged={refresh} /> : null}
            </StepCard>
            <StepCard n={4} id="runners" step={step("runners")}>
              {step("runners") ? <Checks step={step("runners")!} /> : null}
            </StepCard>
            <StepCard n={5} id="optional" step={step("optional")}>
              {step("optional") ? <Checks step={step("optional")!} /> : null}
              <div className="flex flex-wrap gap-2">
                <Link href="/admin/connectors" className={cn(buttonVariants({ variant: "outline", size: "sm" }))}>
                  Connectors
                </Link>
                <Link href="/admin/oauth-clients" className={cn(buttonVariants({ variant: "outline", size: "sm" }))}>
                  OAuth clients
                </Link>
                <Link href="/admin/plugins" className={cn(buttonVariants({ variant: "outline", size: "sm" }))}>
                  Plugins
                </Link>
              </div>
            </StepCard>
            <StepCard n={6} id="try">
              <TryBody />
            </StepCard>
          </ol>
        </>
      ) : null}
    </div>
  );
}
