"use client";

import Link from "next/link";
import { useParams } from "next/navigation";
import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from "react";

import { AccessChoice } from "@/components/accounts/access-choice";
import { AccessDialog } from "@/components/credentials/give-access";
import { WorkspaceAgents } from "@/components/credentials/give-access";
import { ErrorState, Loading } from "@/components/states";
import { Button, buttonVariants } from "@/components/ui/button";
import { ChoiceCards } from "@/components/ui/choice-cards";
import { ArrowLeftIcon, CheckIcon } from "@/components/ui/icons";
import { ErrorNotice, Notice } from "@/components/ui/notice";
import { LEVEL_LABEL, LEVEL_ORDER, preselectAgents } from "@/lib/accounts";
import { savedButText } from "@/lib/account-health";
import { accountKeys, useAccounts, usePutAccess, useTestAccount, type Account, type Level } from "@/lib/api/accounts";
import { appDisplayName, productName } from "@/lib/api/app-name";
import { useApp, useConnectPlan, type AppDetail } from "@/lib/api/apps";
import { errorMessage } from "@/lib/api/errors";
import { useAgents, useOrg as useOrgById } from "@/lib/api/queries";
import { wsRoutes } from "@/lib/routes";
import { cn } from "@/lib/utils";
import { useQueryClient } from "@tanstack/react-query";

import { AppStep, type AppChoice } from "./app-step";
import { BackNext, JourneyFrame, StepFooter } from "./frame";
import {
  doneSentence,
  journeyTitle,
  levelsForApp,
  nextStep,
  prevStep,
  STEP_LABEL,
  stepsOf,
  type JourneyShape,
  type StepId,
} from "./logic";
import { SignInStep, WebsiteStep, type SignInResult } from "./sign-in-step";
import { useDeleteConnection } from "@/lib/api/apps";
import { WebhookAddressStep, WebhookMessageStep, useWebhookDraft } from "@/components/webhook/webhook-connect-step";
import { WebhookDoneTest } from "@/components/webhook/webhook-done-test";

type AgentRef = { id: string; name: string; handle?: string };

interface DoneState {
  accountId: string;
  /** The app's name, or the website's. */
  name: string;
  kind: "connection" | "secret";
  granted: string[];
  grantFailed: string | null;
  missing: string[];
  level: Level | null;
  appId?: string;
  customOnly?: boolean;
  scopes?: string[];
  methodId?: string;
  /** The account's name ("Team calendar"), which Done names. */
  accountName?: string;
  /** The package's suggested first question (C-1031); Done offers it as an unsent chat draft (C-1035). */
  firstPrompt?: string | null;
}

function names(list: string[]): string {
  return list.length <= 1 ? (list[0] ?? "") : `${list.slice(0, -1).join(", ")} and ${list[list.length - 1]}`;
}

/** Back on the left, the step's one action on the right, in the frame's footer (D-388). */
const Footer = BackNext;

function Steps({ steps, current }: { steps: StepId[]; current: StepId }) {
  const at = steps.indexOf(current);
  return (
    <ol className="flex gap-1.5 text-xs font-semibold uppercase tracking-wider" aria-label="Steps">
      {steps.map((s, i) => (
        <li
          key={s}
          aria-current={s === current ? "step" : undefined}
          className={cn("flex-1 border-t-4 pt-1.5", i < at ? "border-accent text-accent-ink" : s === current ? "border-accent text-ink" : "border-line text-muted")}
        >
          {STEP_LABEL[s]}
        </li>
      ))}
    </ol>
  );
}

/**
 * **Give <Agent> <App>** (C-914, `14 §3`): one dialog that ends with an account **and** the agent's access to it, so the
 * agent can use the app the moment it says Done. Steps: App → Agent → Account → Access → Sign in → Done; steps with
 * nothing to ask are skipped. A fixed `agent` (the agent's own page) skips the Agent step; a given `appId` (a deep link,
 * a Connect button) skips the App step.
 */
export function ConnectJourney({
  workspaceId,
  orgId,
  agent,
  appId,
  onClose,
}: {
  workspaceId: string;
  orgId: string;
  agent?: { id: string; name: string };
  appId?: string | null;
  onClose: () => void;
}) {
  const { org, ws } = useParams<{ org: string; ws: string }>();
  const qc = useQueryClient();
  const agents = useAgents(workspaceId);
  const orgRecord = useOrgById(orgId);
  const [choice, setChoice] = useState<AppChoice | null>(appId ? { kind: "app", appId } : null);
  const [stepState, setStepState] = useState<StepId>("app");
  // After the app is chosen the next step waits for what decides it (the app, the accounts, the agents).
  const [advancing, setAdvancing] = useState(!!appId);
  // "Give access" on the App step: straight to the Access step with that account chosen (C-980).
  const [give, setGive] = useState(false);
  const setStep = (s: StepId) => {
    setAdvancing(false);
    setGive(false);
    setStepState(s);
  };
  const [picked, setPicked] = useState<Set<string> | null>(null);
  // The account the person chose; until then the first existing one is recommended (`14 §3` step 2).
  const [accountPick, setAccount] = useState<string | null>(null);
  const [level, setLevel] = useState<Level>("read");
  const [done, setDone] = useState<DoneState | null>(null);
  // (P5-UX-14, C-945) The account is tested once right after connecting; a failure is a warning, the account is kept.
  const testAccount = useTestAccount();
  const [warning, setWarning] = useState<string | null>(null);
  const tested = useRef<string | null>(null);
  const [error, setError] = useState<unknown>(null);
  const [busy, setBusy] = useState(false);
  const [giving, setGiving] = useState(false);
  // A link out of the journey (Try it in chat, the Access tab) closes the dialog at once, without `onClose`: the host's
  // close rewrites the URL (a History write on the page being left) and would undo the link's navigation.
  const [leaving, setLeaving] = useState(false);
  const put = usePutAccess();
  // (P5-REFINE-13, access-4) The Outgoing webhook's Address and Message steps: their state lives here, the footer is the journey's.
  const webhookDraft = useWebhookDraft();
  // "Edit address" after a failed test: the account made a moment ago has the wrong address and goes when the new one is saved.
  const [replacing, setReplacing] = useState<string | null>(null);
  const deleteConnection = useDeleteConnection({ kind: "workspace", id: workspaceId });

  const realApp = choice?.kind === "app" ? choice.appId : undefined;
  const detail = useApp(realApp, workspaceId);
  const plan = useConnectPlan(realApp, workspaceId);
  const accounts = useAccounts({ kind: "workspace", id: workspaceId, agentId: agent?.id });
  // The workspace's agents (with their handles, for **Open chat**); a fixed agent is the only one that counts.
  const agentList: AgentRef[] = useMemo(() => {
    const all: AgentRef[] = agents.data?.items ?? [];
    return agent ? [all.find((a) => a.id === agent.id) ?? agent] : all;
  }, [agent, agents.data]);
  const agentIds = agent ? [agent.id] : [...(picked ?? new Set(preselectAgents(agentList)))];
  const chosenAgents = agentIds.map((id) => agentList.find((a) => a.id === id)).filter((a): a is AgentRef => !!a);

  const existing: Account[] =
    choice?.kind === "app" && agentIds.length > 0
      ? (accounts.data?.items ?? []).filter(
          (a) =>
            a.kind === "app" &&
            a.app?.app_id === choice.appId &&
            a.can.give_access &&
            a.given !== true &&
            !agentIds.every((id) => a.agents.some((x) => x.agent.id === id)),
        )
      : [];
  const account = accountPick ?? existing[0]?.ref.id ?? "new";
  const useExisting = account !== "new" && existing.some((a) => a.ref.id === account);
  const target = useExisting ? existing.find((a) => a.ref.id === account) : undefined;
  const app: AppDetail | undefined = detail.data;
  const levels: Level[] = target ? LEVEL_ORDER.filter((l) => target.levels.includes(l)) : app ? levelsForApp(app) : [];
  const effective: Level = levels.includes(level) ? level : (levels[0] ?? "read");

  const shape: JourneyShape = {
    appGiven: !!appId,
    agentFixed: !!agent,
    noAgents: !agent && !agents.isPending && agentList.length === 0,
    website: choice?.kind === "website",
    hasExisting: existing.length > 0,
    useExisting,
    hasChoice: levels.length > 0,
    noAgentChosen: agentIds.length === 0,
    webhook: choice?.kind === "app" && choice.appId === "webhook",
  };
  const steps = stepsOf(shape);

  const ready = choice?.kind === "website" ? !agents.isPending : !!choice && !!app && !accounts.isPending && !agents.isPending;
  const step: StepId = advancing && ready ? (give && steps.includes("access") ? "access" : stepAfterApp(shape)) : stepState;
  useEffect(() => {
    if (step !== "done" || !done || done.kind !== "connection" || tested.current === done.accountId) return;
    tested.current = done.accountId;
    // A method with a probe was checked while connecting; only the others are tested here (D-335).
    if (app?.connect.find((m) => m.id === done.methodId)?.has_probe) return;
    testAccount.mutate(
      { ref: { type: "connection", id: done.accountId } },
      {
        onSuccess: (a) =>
          setWarning(
            a.result === "unreachable"
              ? savedButText(done.name, a.detail?.host ?? undefined)
              : a.result === "rejected"
                ? `Saved, but ${done.name} did not accept it. Check it or test again.`
                : null,
          ),
      },
    );
    // eslint-disable-next-line react-hooks/exhaustive-deps -- once per account
  }, [step, done]);
  const waiting = advancing && !ready;

  const refresh = useCallback(() => {
    void qc.invalidateQueries({ queryKey: accountKeys.all });
    void qc.invalidateQueries({ queryKey: ["connections"] });
    void qc.invalidateQueries({ queryKey: ["apps"] });
  }, [qc]);

  const appName = app ? appDisplayName(app) : "";
  const agentWord = names(chosenAgents.map((a) => a.name));
  // The title carries no host: "Give Daneel an app", then "GitHub for Daneel" (the Sign in step shows the deployment).
  const title = journeyTitle({
    step,
    app: app ? productName(app) : undefined,
    agent: agent?.name ?? agentWord,
    website: choice?.kind === "website",
  });

  function back() {
    const p = prevStep(shape, step);
    if (p) setStep(p);
  }

  async function saveExisting() {
    if (!target) return;
    setBusy(true);
    setError(null);
    try {
      for (const id of agentIds) {
        await put.mutateAsync({ agentId: id, connectionId: target.ref.id, body: { level: levels.length > 0 ? effective : "read" } });
      }
      refresh();
      setDone({
        accountId: target.ref.id,
        name: appName,
        kind: "connection",
        granted: agentIds,
        grantFailed: null,
        missing: [],
        level: levels.length > 0 ? effective : "read",
        appId: target.app?.app_id,
        accountName: target.name,
        firstPrompt: app?.first_prompt ?? null,
      });
      setStep("done");
    } catch (err) {
      setError(err);
    } finally {
      setBusy(false);
    }
  }

  const agentKey = agentIds.join(",");
  const hasLevels = levels.length > 0;
  const customOnly = app?.app_restrictions?.custom_only === true;
  const finishSignIn = (r: SignInResult) => {
    if (replacing && replacing !== r.accountId) deleteConnection.mutate(replacing);
    setReplacing(null);
    refresh();
    setDone({
      accountId: r.accountId,
      name: appName,
      kind: "connection",
      granted: r.granted.length > 0 ? r.granted : r.grantFailed ? [] : agentKey ? agentKey.split(",") : [],
      grantFailed: r.grantFailed,
      missing: r.missing,
      // A one-level app (no write tools) is read only; the Done line says so (B5). Custom-only apps have no single level.
      level: hasLevels ? effective : customOnly ? null : "read",
      appId: realApp,
      customOnly,
      scopes: r.scopes,
      methodId: r.methodId,
      accountName: r.accountName,
      firstPrompt: app?.first_prompt ?? null,
    });
    setStep("done");
  };

  const afterNext = (from: StepId) => {
    setError(null);
    setStep(nextStep(shape, from));
  };

  async function retryGrant() {
    if (!done) return;
    setBusy(true);
    setError(null);
    try {
      for (const id of agentIds) await put.mutateAsync({ agentId: id, connectionId: done.accountId, body: { level: done.level ?? "read" } });
      setDone({ ...done, granted: agentIds, grantFailed: null });
    } catch (err) {
      setError(err);
    } finally {
      setBusy(false);
    }
  }

  let body: ReactNode;
  if (choice?.kind === "app" && detail.isError) {
    body = <ErrorState error={detail.error} onRetry={() => void detail.refetch()} />;
  } else if (waiting) {
    body = <Loading />;
  } else if (step === "app") {
    body = (
      <AppStep
        workspaceId={workspaceId}
        orgId={orgId}
        agent={agent}
        onPick={(c) => {
          setChoice(c);
          setAccount(null);
          setGive(false);
          setAdvancing(true);
        }}
        onGive={(id, accountId) => {
          setChoice({ kind: "app", appId: id });
          setAccount(accountId);
          setGive(true);
          setAdvancing(true);
        }}
      />
    );
  } else if (!choice || (choice.kind === "app" && (detail.isPending || agents.isPending || accounts.isPending))) {
    body = <Loading />;
  } else if (step === "agent") {
    body = (
      <div className="space-y-3">
        <fieldset className="min-w-0 space-y-2">
          <legend className="mb-1 text-sm font-semibold text-ink">Which agents can use {choice.kind === "website" ? "it" : appName}?</legend>
          <WorkspaceAgents
            wsId={workspaceId}
            register={() => undefined}
            has={new Set()}
            selected={new Set(agentIds)}
            toggle={(id) => {
              const next = new Set(agentIds);
              if (next.has(id)) next.delete(id);
              else next.add(id);
              setPicked(next);
            }}
            disabled={false}
            idp="cj-agent"
          />
          <p className="text-xs text-muted">You can choose none now and give access later.</p>
        </fieldset>
        <Footer
          onBack={prevStep(shape, "agent") ? () => back() : undefined}
          onNext={() => afterNext("agent")}
          nextLabel={agentIds.length === 1 ? `Give ${chosenAgents[0]?.name ?? "the agent"} access` : agentIds.length > 1 ? "Give access" : "Next"}
        />
      </div>
    );
  } else if (step === "account") {
    body = (
      <div className="space-y-3">
        <ChoiceCards<string>
          legend={`Which ${appName} account?`}
          value={useExisting ? account : "new"}
          onChange={setAccount}
          options={[
            ...existing.map((a, i) => ({
              value: a.ref.id,
              label: `Use ${a.name}${a.connected_by ? ` (connected by ${a.connected_by.name})` : ""}`,
              badge: i === 0 ? "Recommended" : undefined,
            })),
            { value: "new", label: "Connect another account" },
          ]}
        />
        <ErrorNotice error={error} />
        <Footer
          onBack={back}
          onNext={useExisting && levels.length === 0 ? () => void saveExisting() : () => afterNext("account")}
          nextLabel={useExisting && levels.length === 0 ? `Give ${agentWord} access` : "Next"}
          busy={busy}
        />
      </div>
    );
  } else if (step === "access") {
    const who = chosenAgents.length === 1 ? chosenAgents[0]!.name : "These agents";
    body = (
      <div className="space-y-3">
        <p className="text-sm font-medium text-ink">What may {agentWord || "the agent"} do in {appName}?</p>
        <AccessChoice
          account={{ levels, limit: target?.limit, app: target?.app, managed_by: target?.managed_by }}
          agentName={who}
          orgName={orgRecord.data?.name}
          value={effective}
          onChange={setLevel}
          legend={`What may ${who} do in ${appName}?`}
        />
        <p className="text-xs text-muted">You can change this at any time on {chosenAgents.length === 1 ? `${who}'s` : "each agent's"} Apps &amp; access tab.</p>
        <ErrorNotice error={error} />
        <Footer
          onBack={back}
          onNext={useExisting ? () => void saveExisting() : () => afterNext("access")}
          nextLabel={useExisting ? `Give ${agentWord} access` : "Next"}
          busy={busy}
        />
      </div>
    );
  } else if (step === "signin" && choice.kind === "website") {
    body = (
      <WebsiteStep
        workspaceId={workspaceId}
        agentIds={agentIds}
        onBack={prevStep(shape, "signin") ? back : undefined}
        onResult={(r) => {
          refresh();
          setDone({ accountId: r.accountId, name: r.name, kind: "secret", granted: agentIds, grantFailed: null, missing: [], level: null });
          setStep("done");
        }}
      />
    );
  } else if ((step === "address" || step === "message") && app) {
    const ctx = {
      app,
      workspaceId,
      orgId,
      level: levels.length > 0 ? effective : null,
      agentIds,
      hasWriteTools: app.tools.some((t) => t.group === "write"),
      goTo: (s: "address" | "message") => setStep(s),
      onResult: finishSignIn,
    };
    body =
      step === "address" ? (
        <WebhookAddressStep draft={webhookDraft} agentName={agent?.name ?? agentWord} onBack={prevStep(shape, "address") ? back : undefined} onNext={() => setStep("message")} />
      ) : (
        <WebhookMessageStep draft={webhookDraft} ctx={ctx} onBack={back} />
      );
  } else if (step === "signin" && app) {
    body = plan.isPending ? (
      <Loading />
    ) : plan.isError ? (
      <ErrorState error={plan.error} onRetry={() => void plan.refetch()} />
    ) : (
      <SignInStep
        key={app.app_id}
        app={app}
        plan={plan.data}
        workspaceId={workspaceId}
        orgId={orgId}
        level={levels.length > 0 ? effective : null}
        agentIds={agentIds}
        fixedInstanceId={choice.kind === "app" ? choice.instanceId : undefined}
        hasWriteTools={app.tools.some((t) => t.group === "write")}
        onBack={prevStep(shape, "signin") ? back : undefined}
        onResult={finishSignIn}
      />
    );
  } else if (step === "done" && done) {
    const granted = done.granted.map((id) => agentList.find((a) => a.id === id)).filter((a): a is AgentRef => !!a);
    const who = names(granted.map((a) => a.name));
    const first = granted[0];
    const accountName = done.accountName ?? done.name;
    const accessTab = first && org && ws ? wsRoutes(org, ws).agent(first.handle ?? first.id, { tab: "access" }) : null;
    // (C-1035) The suggested first question opens as an unsent draft in a new conversation; at most 500 characters.
    const prompt = done.firstPrompt?.trim() ? done.firstPrompt.trim().slice(0, 500) : null;
    const chat = first && org && ws ? wsRoutes(org, ws).agent(first.handle ?? first.id, prompt ? { tab: "chat", draft: prompt } : { tab: "chat" }) : null;
    body = (
      <div role="status" className="space-y-4 text-sm">
        {done.grantFailed ? (
          <>
            <p className="font-medium text-ink">
              {done.name} is connected, but {agentWord || "the agent"} could not be given access.
            </p>
            <Notice tone="pending" role="alert">
              {errorMessage({ code: done.grantFailed })} <span className="text-ink-2">The account is kept; you can try the access again.</span>
            </Notice>
            <ErrorNotice error={error} />
            <Button size="sm" disabled={busy} onClick={() => void retryGrant()}>
              {busy ? "Saving…" : "Give access"}
            </Button>
          </>
        ) : (
          <div className="flex flex-col items-center gap-3 pt-2 text-center">
            <span aria-hidden="true" className="inline-flex size-12 items-center justify-center rounded-full bg-accent-soft text-accent-ink">
              <CheckIcon className="size-6" />
            </span>
            <p className="text-base font-semibold text-ink">
              {doneSentence({ agent: who, app: granted.length > 0 ? accountName : done.name, level: done.level, agentChosen: granted.length > 0 })}
            </p>
            {granted.length > 0 ? (
              <p className="text-ink-2">
                {done.level ? `${LEVEL_LABEL[done.level]}. ` : ""}You can change this on{" "}
                {accessTab ? (
                  <Link href={accessTab} onClick={() => setLeaving(true)} className="font-medium text-link underline">
                    {who}&apos;s Apps &amp; access tab
                  </Link>
                ) : (
                  `${who}'s Apps & access tab`
                )}
                .
              </p>
            ) : null}
          </div>
        )}
        {warning ? <Notice tone="pending" role="alert">{warning}</Notice> : null}
        {done.missing.length > 0 ? <Notice tone="pending">Some requested permissions were not granted: {done.missing.join(", ")}.</Notice> : null}
        {!done.grantFailed && done.appId === "webhook" ? (
          <WebhookDoneTest
            connectionId={done.accountId}
            onEditAddress={() => {
              setReplacing(done.accountId);
              setDone(null);
              setStep("address");
            }}
          />
        ) : null}
        {!done.grantFailed && prompt && first ? (
          <div className="rounded-control bg-subtle p-3">
            <p className="text-xs font-semibold uppercase tracking-wider text-muted">Try it</p>
            <p className="mt-1 text-ink">&ldquo;{prompt}&rdquo;</p>
          </div>
        ) : null}
        <StepFooter>
          <Button type="button" variant="ghost" onClick={onClose}>
            Close
          </Button>
          {!done.grantFailed ? (
            <>
              <span className="flex-1" />
              {first ? (
                <Button
                  type="button"
                  variant="outline"
                  onClick={() => {
                    setChoice(null);
                    setDone(null);
                    setPicked(null);
                    setAccount(null);
                    setStep("app");
                  }}
                >
                  Another app
                </Button>
              ) : (
                <Button type="button" variant="outline" onClick={() => setGiving(true)}>
                  Give access
                </Button>
              )}
              {chat ? (
                <Link className={cn(buttonVariants({ size: "lg" }), "max-sm:order-first max-sm:w-full")} href={chat} onClick={() => setLeaving(true)}>
                  {prompt ? "Try it in chat" : "Open chat"}
                </Link>
              ) : null}
            </>
          ) : null}
        </StepFooter>
        {giving ? (
          <AccessDialog
            open
            onOpenChange={setGiving}
            title={done.name}
            subject={{
              target: { kind: done.kind, id: done.accountId },
              scope: { kind: "workspace", id: workspaceId },
              orgId,
              appId: done.appId,
              appWorkspaceId: workspaceId,
              customOnly: done.customOnly,
              scopes: done.scopes,
              connectMethod: done.methodId,
            }}
          />
        ) : null}
      </div>
    );
  } else {
    body = <Loading />;
  }

  if (leaving) return null;
  const shown = steps.filter((s) => s !== "agent" || !agent);
  const previous = prevStep(shape, step);
  return (
    <JourneyFrame
      title={title}
      testId="ws-connect-journey"
      stepper={<Steps steps={shown} current={step} />}
      footer={step !== "app"}
      onClose={onClose}
      back={
        previous && step !== "done" ? (
          <button
            type="button"
            aria-label="Back"
            onClick={back}
            className="inline-flex size-9 items-center justify-center rounded-control text-ink-2 hover:bg-subtle hover:text-ink sm:hidden pointer-coarse:size-11"
          >
            <ArrowLeftIcon className="size-5" />
          </button>
        ) : null
      }
    >
      {body}
    </JourneyFrame>
  );
}

/** The first step after the app is chosen. */
function stepAfterApp(shape: JourneyShape): StepId {
  const steps = stepsOf({ ...shape, appGiven: false });
  return steps[1] ?? "done";
}
