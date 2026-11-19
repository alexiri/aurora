"use client";

import Link from "next/link";
import { useSearchParams } from "next/navigation";
import { useQueryClient } from "@tanstack/react-query";
import { useMemo, useState, type ReactNode } from "react";

import { ConnectorAvailableCallout } from "@/components/connectors/available-callout";
import { LlmKeyForm } from "@/components/credentials/llm-form";
import { ErrorState, Loading } from "@/components/states";
import { Badge } from "@/components/ui/badge";
import { Button, buttonVariants } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { CheckboxField } from "@/components/ui/form";
import { Notice } from "@/components/ui/notice";
import { PageHeader } from "@/components/ui/page-header";
import { StatusDot } from "@/components/ui/status-dot";
import { errorMessage } from "@/lib/api/errors";
import { onboardingKeys, useAdminSetup, type SetupCheck, type SetupStatus, type SetupStep } from "@/lib/api/onboarding";
import { formatBytes } from "@/lib/api/attachments";
import { useAdminStorage } from "@/lib/api/org";
import { useBackfillStarterGrants, usePlacement, useSavePlacement, type PlacementView as Placement, type PutPlacement } from "@/lib/api/placement";
import { useSecrets } from "@/lib/api/secrets";
import { useOrgs, useWorkspaces } from "@/lib/api/queries";
import { landingPath } from "@/lib/nav/landing";
import { adminRoutes } from "@/lib/routes";
import { cn } from "@/lib/utils";

import { OwnerlessBanner } from "./ownerless-banner";
import { PlacementView } from "./placement-view";
import { SignInGaps, gapsFromChecks } from "./sign-in-gaps";
import { PreviewsLine } from "./previews-line";
import { PushLine } from "./push-line";
import { SshLine } from "./ssh-line";

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
  no_provider: "Add an OIDC provider so people can sign in. The operator guide on sign-in explains how.",
  no_role_mappings: "Map a role from your sign-in token to instance admin. The operator guide on sign-in explains how.",
  no_endpoints: "Add an LLM endpoint under Admin › Settings › LLM endpoints. The operator guide on LLM endpoints explains how.",
  endpoint_unreachable: "Check the endpoint's address and that the API can reach it. Test connection is on the endpoint's page.",
  no_runner: "Install a runner and register it. The operator guide on runners explains how.",
  runner_offline: "Check that the runner is running and can reach the API. The operator guide on runners explains how.",
  no_gateway: "Start the gateway. Without it agents cannot reach the internet or the LLM.",
  plugins_pending: "Review them under Apps & accounts › Apps.",
  email_not_configured: "Set an SMTP server under Platform › Email.",
};

/** The checks whose fix is made on Admin › Settings › LLM endpoints (SET-3): the step links there. */
const LLM_SETTINGS_FIXES: Record<string, true> = { no_endpoints: true, endpoint_unreachable: true };

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
      {fix ? (
        <p className="text-ink-2">
          {fix}
          {c.reason in LLM_SETTINGS_FIXES ? (
            <>
              {" "}
              <Link href={adminRoutes.aiModels("endpoints")} className="font-medium text-ink underline underline-offset-2">
                Open LLM endpoints
              </Link>
            </>
          ) : null}
        </p>
      ) : null}
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
      <Link href={adminRoutes.signIn()} className={cn(buttonVariants({ variant: "outline", size: "sm" }))}>
        Open Sign-in settings
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

/** Placement allows starter grants: a saved shared organization with personal workspaces (C-521). */
const startersAllowed = (p: Placement) => p.saved && p.mode === "shared_org" && p.personal_workspace.enabled;

/** "Give it to every new user's agent": the starter grant of the org key (`04 §New-user placement`), on by default (C-922). */
function StarterSwitch({ placement, secretId, onChanged }: { placement: Placement; secretId: string; onChanged: () => void }) {
  const save = useSavePlacement();
  const on = placement.starter_grants.some((g) => g.kind === "secret" && g.id === secretId);
  const usable = startersAllowed(placement);
  function toggle(next: boolean) {
    const others = placement.starter_grants.filter((g) => !(g.kind === "secret" && g.id === secretId)).map((g) => ({ kind: g.kind, id: g.id }));
    save.mutate(putWithStarters(placement, next ? [...others, { kind: "secret", id: secretId }] : others), { onSuccess: onChanged });
  }
  return (
    <div className="space-y-2">
      <CheckboxField
        id="setup-starter"
        label="Give the organization's AI key to every new person's agent"
        hint="Every new person's agent can use this key from the first minute. Turn off to make people add their own."
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

/** (C-922) People who signed in before the starter grant existed: one click gives them the key too. Idempotent. */
function BackfillOffer({ waiting }: { waiting: number }) {
  const backfill = useBackfillStarterGrants();
  if (backfill.data) {
    return (
      <Notice tone="accent" role="status">
        Done: {backfill.data.granted === 1 ? "1 agent" : `${backfill.data.granted} agents`} can use the key now
        {backfill.data.skipped > 0 ? ` (${backfill.data.skipped} already could or could not be given it).` : "."}
      </Notice>
    );
  }
  if (waiting <= 0) return null;
  return (
    <div className="space-y-2">
      <p className="text-sm text-ink-2">
        {waiting === 1 ? "1 person signed in before and their agent can't use the key yet." : `${waiting} people signed in before and their agents can't use the key yet.`}
      </p>
      <Button variant="outline" size="sm" disabled={backfill.isPending} onClick={() => backfill.mutate()}>
        Also give it to the {waiting} {waiting === 1 ? "person" : "people"} who signed in before
      </Button>
      {backfill.error ? (
        <Notice tone="danger" role="alert">
          {errorMessage(backfill.error)}
        </Notice>
      ) : null}
    </div>
  );
}

function LlmBody({ step, waiting, onChanged }: { step: SetupStep; waiting: number; onChanged: () => void }) {
  const placement = usePlacement();
  const savePlacement = useSavePlacement();
  const org = placement.data?.default_org ?? null;
  const secrets = useSecrets({ kind: "organization", id: org?.id ?? "" });
  const orgKey = (secrets.data?.items ?? []).find((s) => s.kind === "llm" && s.status === "active");
  return (
    <div className="space-y-4">
      <Checks step={step} />
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
            <LlmKeyForm
              scope={{ kind: "organization", id: org.id }}
              orgId={org.id}
              onSaved={(secret) => {
                // (C-922) The first key is a starter grant by default, whenever placement allows one.
                const p = placement.data;
                if (p && startersAllowed(p)) {
                  const others = p.starter_grants.map((g) => ({ kind: g.kind, id: g.id }));
                  savePlacement.mutate(putWithStarters(p, [...others, { kind: "secret", id: secret.id }]), { onSettled: onChanged });
                } else onChanged();
              }}
            />
          )}
          {orgKey ? <StarterSwitch placement={placement.data} secretId={orgKey.id} onChanged={onChanged} /> : null}
          {orgKey && placement.data.starter_grants.some((g) => g.kind === "secret" && g.id === orgKey.id) ? <BackfillOffer waiting={waiting} /> : null}
        </div>
      )}
      <Link href={adminRoutes.aiModels("endpoints")} className="inline-block text-sm font-medium text-ink underline underline-offset-2">
        Open AI models
      </Link>
    </div>
  );
}

/** Step 5: apps people can't sign in to yet (C-924), chat platforms and e-mail, each linking to the page that holds it. */
function OptionalBody({ step }: { step: SetupStep }) {
  const placement = usePlacement();
  const gaps = gapsFromChecks(step.checks);
  // The instance-wide OAuth client count is the gaps' business now; the rest stays a plain check.
  const rest: SetupStep = { ...step, checks: step.checks.filter((c) => !c.id.startsWith("app:") && c.id !== "oauth_clients") };
  return (
    <div className="space-y-4">
      <SignInGaps gaps={gaps} where={{ level: "admin", orgSlug: placement.data?.default_org?.slug }} />
      <Checks step={rest} />
      <div className="flex flex-wrap gap-2">
        <Link href={adminRoutes.chatPlatforms()} className={cn(buttonVariants({ variant: "outline", size: "sm" }))}>
          Chat platforms
        </Link>
        <Link href={adminRoutes.platform("email")} className={cn(buttonVariants({ variant: "outline", size: "sm" }))}>
          Platform › Email
        </Link>
        <Link href={adminRoutes.apps("apps")} className={cn(buttonVariants({ variant: "outline", size: "sm" }))}>
          Apps &amp; accounts
        </Link>
      </div>
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

/** Setup is complete when no step that blocks is still to do (`optional` never blocks). */
export function setupComplete(setup: SetupStatus): boolean {
  return setup.placement_saved && setup.steps.every((s) => s.id === "optional" || s.status !== "todo");
}

/** The count of a step's checks by reason, for the health lines. */
function checkOf(setup: SetupStatus, stepId: string, id: string) {
  return setup.steps.find((s) => s.id === stepId)?.checks.find((c) => c.id === id);
}

function HealthLine({ tone, title, children, href, hrefLabel }: { tone: Tone; title: string; children: ReactNode; href?: string; hrefLabel?: string }) {
  return (
    <li className="flex flex-wrap items-center gap-x-4 gap-y-1 px-4 py-3 text-sm" data-health={title}>
      <StatusDot tone={tone === "neutral" ? "neutral" : tone} className="min-w-28 text-sm font-semibold text-ink">
        {title}
      </StatusDot>
      <span className="min-w-0 flex-1 text-ink-2">{children}</span>
      {href ? (
        <Link href={href} className="text-sm font-medium text-ink underline underline-offset-2">
          {hrefLabel}
        </Link>
      ) : null}
    </li>
  );
}

/** After setup: what is healthy and what needs the admin, from the same status as the checklist (`14 §9`). */
function HealthView({ setup, onRunSetup }: { setup: SetupStatus; onRunSetup: () => void }) {
  const placement = usePlacement();
  const storage = useAdminStorage();
  const optional = setup.steps.find((s) => s.id === "optional");
  const gaps = gapsFromChecks(optional?.checks ?? []);
  const blocked = gaps.filter((g) => !g.tokenOnly);
  const noMail = optional?.checks.some((c) => c.reason === "email_not_configured") ?? false;
  const orgSlug = placement.data?.default_org?.slug;
  const runners = setup.steps.find((s) => s.id === "runners");
  const llm = setup.steps.find((s) => s.id === "llm");
  const endpoints = llm?.checks.filter((c) => c.id.startsWith("endpoint:")) ?? [];
  const down = endpoints.filter((c) => c.status !== "ok");
  const gateway = checkOf(setup, "runners", "gateway");
  const runner = checkOf(setup, "runners", "runners");
  const tone = (s?: SetupCheck["status"]): Tone => (s === "ok" ? "accent" : s === "warning" ? "pending" : "neutral");
  const needs = blocked.length + (noMail ? 1 : 0);
  return (
    <div className="space-y-6" data-testid="admin-health">
      <header className="flex flex-wrap items-end justify-between gap-4">
        <p className="max-w-3xl font-display text-xl font-medium text-ink">
          Aurora is running.
          {needs > 0
            ? ` ${[
                blocked.length > 0 ? `People can't sign in to ${blocked.length === 1 ? "1 app" : `${blocked.length} apps`} yet` : "",
                noMail ? "e-mail is not set up" : "",
              ]
                .filter(Boolean)
                .join(", and ")
                .replace(/^e-mail/, "E-mail")}.`
            : " Nothing needs you."}
        </p>
        <Button variant="outline" onClick={onRunSetup}>
          Run setup again
        </Button>
      </header>
      {needs > 0 ? (
        <section aria-labelledby="needs-you-h" className="space-y-4 rounded-card border border-pending-line bg-pending-panel p-4">
          <div>
            <h2 id="needs-you-h" className="font-display text-base font-semibold text-ink">
              Needs you
            </h2>
            <p className="text-sm text-ink-2">Each item says what is missing and opens the page that fixes it.</p>
          </div>
          <SignInGaps gaps={blocked} where={{ level: "admin", orgSlug }} />
          {noMail ? (
            <div className="flex flex-wrap items-center gap-x-4 gap-y-2 text-sm" data-reason="email_not_configured">
              <div className="min-w-0 flex-1">
                <p className="font-semibold text-ink">E-mail is not set up</p>
                <p className="text-ink-2">E-mail is not set up. Aurora cannot send approval or invitation e-mails.</p>
              </div>
              <Link href={adminRoutes.platform("email")} className={cn(buttonVariants({ variant: "outline", size: "sm" }))}>
                Open Platform › Email
              </Link>
            </div>
          ) : null}
        </section>
      ) : null}
      {gaps.some((g) => g.tokenOnly) ? <SignInGaps gaps={gaps.filter((g) => g.tokenOnly)} where={{ level: "admin", orgSlug }} /> : null}
      <Card className="p-0">
        <h2 className="px-4 pt-4 font-display text-base font-semibold text-ink">Health</h2>
        <ul aria-label="Health" className="divide-y divide-line">
          <HealthLine tone={tone(runner?.status)} title="Runners" href={adminRoutes.platform("runners")} hrefLabel="Open Platform › Runners">
            {runner?.message ?? runners?.reason}
          </HealthLine>
          <HealthLine tone={tone(gateway?.status)} title="Gateway">
            {gateway?.message ?? "No gateway has been seen."}
          </HealthLine>
          <HealthLine tone={endpoints.length === 0 ? "neutral" : down.length > 0 ? "pending" : "accent"} title="AI models" href={adminRoutes.aiModels("endpoints")} hrefLabel="Open AI models">
            {endpoints.length === 0
              ? "No AI endpoint is set up."
              : down.length > 0
                ? down.map((c) => c.message).join(" ")
                : `${endpoints.length === 1 ? "1 endpoint answers" : `${endpoints.length} endpoints answer`}.`}
          </HealthLine>
          <HealthLine tone="accent" title="New people" href={adminRoutes.people("new-users")} hrefLabel="Open People › New users">
            {setup.placement_saved ? "Land where you chose, with a personal workspace and the organization's AI key." : "Not saved yet."}
          </HealthLine>
          {storage.data ? (
            <HealthLine tone="accent" title="Storage" href={adminRoutes.data()} hrefLabel="Open Data">
              {formatBytes(storage.data.blob_bytes)} in {storage.data.blob_count} stored files.
            </HealthLine>
          ) : null}
        </ul>
      </Card>
      <div className="space-y-3">
        <PreviewsLine />
        <SshLine />
        <PushLine />
      </div>
    </div>
  );
}

/**
 * Administration › Overview (`14 §Agents, accounts and access` §9): the setup checklist until setup is complete,
 * then a health page from the same status. **Run setup again** brings the checklist back.
 */
export function SetupView() {
  const qc = useQueryClient();
  const setup = useAdminSetup();
  const params = useSearchParams();
  const [again, setAgain] = useState(params.get("setup") === "1");
  const refresh = () => void qc.invalidateQueries({ queryKey: onboardingKeys.setup });
  const step = (id: string) => setup.data?.steps.find((s) => s.id === id);
  const complete = setup.data ? setupComplete(setup.data) : false;

  if (setup.data && complete && !again) {
    return (
      <div className="max-w-page space-y-6">
        <PageHeader title="Overview" description="What needs you, and whether Aurora is healthy." className="mb-0" />
        <OwnerlessBanner />
        <ConnectorAvailableCallout />
        <HealthView setup={setup.data} onRunSetup={() => setAgain(true)} />
      </div>
    );
  }

  return (
    <div className="max-w-page space-y-6">
      <PageHeader
        title="Set up Aurora"
        description="Six steps from a fresh install to your first answer. Each shows what is ready now."
        className="mb-0"
        actions={
          complete ? (
            <Button variant="outline" onClick={() => setAgain(false)}>
              Back to the health page
            </Button>
          ) : undefined
        }
      />
      {setup.isPending ? <Loading /> : null}
      {setup.isError ? <ErrorState error={setup.error} onRetry={() => setup.refetch()} /> : null}
      <OwnerlessBanner />
      <ConnectorAvailableCallout />
      <PreviewsLine />
      <SshLine />
      <PushLine />
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
              <Link href={adminRoutes.people("new-users")} className="inline-block text-sm font-medium text-ink underline underline-offset-2">
                Open People › New users
              </Link>
            </StepCard>
            <StepCard n={3} id="llm" step={step("llm")}>
              {step("llm") ? <LlmBody step={step("llm")!} waiting={setup.data.personal_workspaces_without_starter ?? 0} onChanged={refresh} /> : null}
            </StepCard>
            <StepCard n={4} id="runners" step={step("runners")}>
              {step("runners") ? <Checks step={step("runners")!} /> : null}
            </StepCard>
            <StepCard n={5} id="optional" step={step("optional")}>
              {step("optional") ? <OptionalBody step={step("optional")!} /> : null}
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
