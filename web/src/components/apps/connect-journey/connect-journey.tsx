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
import { Dialog, DialogContent } from "@/components/ui/dialog";
import { Notice } from "@/components/ui/notice";
import { LEVEL_ORDER, preselectAgents } from "@/lib/accounts";
import { savedButText } from "@/lib/account-health";
import { accountKeys, useAccounts, usePutAccess, useTestAccount, type Account, type Level } from "@/lib/api/accounts";
import { appDisplayName } from "@/lib/api/app-name";
import { useApp, useConnectPlan, type AppDetail } from "@/lib/api/apps";
import { errorMessage } from "@/lib/api/errors";
import { useAgents, useOrg as useOrgById } from "@/lib/api/queries";
import { wsRoutes } from "@/lib/routes";
import { cn } from "@/lib/utils";
import { useQueryClient } from "@tanstack/react-query";

import { AppStep, type AppChoice } from "./app-step";
import {
  doneSentence,
  levelsForApp,
  nextStep,
  prevStep,
  STEP_LABEL,
  stepsOf,
  type JourneyShape,
  type StepId,
} from "./logic";
import { SignInStep, WebsiteStep, type SignInResult } from "./sign-in-step";

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
}

function names(list: string[]): string {
  return list.length <= 1 ? (list[0] ?? "") : `${list.slice(0, -1).join(", ")} and ${list[list.length - 1]}`;
}

function Footer({ onBack, onNext, nextLabel = "Next", nextDisabled, busy }: { onBack?: () => void; onNext?: () => void; nextLabel?: string; nextDisabled?: boolean; busy?: boolean }) {
  return (
    <div className="flex flex-wrap justify-end gap-2 pt-2">
      {onBack ? (
        <Button type="button" variant="ghost" onClick={onBack}>
          Back
        </Button>
      ) : null}
      {onNext ? (
        <Button type="button" size="lg" disabled={nextDisabled || busy} onClick={onNext}>
          {busy ? "Saving…" : nextLabel}
        </Button>
      ) : null}
    </div>
  );
}

function Steps({ steps, current }: { steps: StepId[]; current: StepId }) {
  const at = steps.indexOf(current);
  return (
    <ol className="mb-4 flex gap-1.5 text-xs font-semibold uppercase tracking-wider" aria-label="Steps">
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
  const setStep = (s: StepId) => {
    setAdvancing(false);
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
  const put = usePutAccess();

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
  };
  const steps = stepsOf(shape);

  const ready = choice?.kind === "website" ? !agents.isPending : !!choice && !!app && !accounts.isPending && !agents.isPending;
  const step: StepId = advancing && ready ? stepAfterApp(shape) : stepState;
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
  const title =
    choice?.kind === "website"
      ? agentWord
        ? `Give ${agentWord} a website`
        : "Connect a website"
      : appName
        ? agentWord
          ? `Give ${agentWord} ${appName}`
          : `Connect ${appName}`
        : agentWord
          ? `Give ${agentWord} an app`
          : "Connect an app or website";

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
        level: levels.length > 0 ? effective : null,
        appId: target.app?.app_id,
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
    refresh();
    setDone({
      accountId: r.accountId,
      name: appName,
      kind: "connection",
      granted: r.granted.length > 0 ? r.granted : r.grantFailed ? [] : agentKey ? agentKey.split(",") : [],
      grantFailed: r.grantFailed,
      missing: r.missing,
      level: hasLevels ? effective : null,
      appId: realApp,
      customOnly,
      scopes: r.scopes,
      methodId: r.methodId,
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
        onPick={(c) => {
          setChoice(c);
          setAccount(null);
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
        {error ? <Notice tone="danger" role="alert">{errorMessage(error)}</Notice> : null}
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
        <p className="text-xs text-muted">You can change this at any time on {chosenAgents.length === 1 ? `${who}'s` : "each agent's"} Access tab.</p>
        {error ? <Notice tone="danger" role="alert">{errorMessage(error)}</Notice> : null}
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
    body = (
      <div role="status" className="space-y-3 text-sm">
        {done.grantFailed ? (
          <>
            <p className="font-medium text-ink">
              {done.name} is connected, but {agentWord || "the agent"} could not be given access.
            </p>
            <Notice tone="pending" role="alert">
              {errorMessage({ code: done.grantFailed })} <span className="text-ink-2">The account is kept; you can try the access again.</span>
            </Notice>
            {error ? <Notice tone="danger" role="alert">{errorMessage(error)}</Notice> : null}
            <Button size="sm" disabled={busy} onClick={() => void retryGrant()}>
              {busy ? "Saving…" : "Give access"}
            </Button>
          </>
        ) : (
          <p className="text-base font-medium text-ink">
            {doneSentence({ agent: who, app: done.name, level: done.level, agentChosen: granted.length > 0 })}
          </p>
        )}
        {warning ? <Notice tone="pending" role="alert">{warning}</Notice> : null}
        {done.missing.length > 0 ? <Notice tone="pending">Some requested permissions were not granted: {done.missing.join(", ")}.</Notice> : null}
        {!done.grantFailed ? (
          <div className="flex flex-wrap gap-2 pt-1">
            {first && org && ws ? (
              <Link className={buttonVariants({ size: "sm" })} href={wsRoutes(org, ws).agent(first.handle ?? first.id)} onClick={onClose}>
                Open chat
              </Link>
            ) : null}
            {first ? (
              <Button
                size="sm"
                variant="outline"
                onClick={() => {
                  setChoice(null);
                  setDone(null);
                  setPicked(null);
                  setAccount(null);
                  setStep("app");
                }}
              >
                Give {who} another app
              </Button>
            ) : (
              <Button size="sm" onClick={() => setGiving(true)}>
                Give access
              </Button>
            )}
            <Button size="sm" variant="ghost" onClick={onClose}>
              Done
            </Button>
          </div>
        ) : null}
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

  return (
    <Dialog open onOpenChange={(o) => (o ? null : onClose())}>
      <DialogContent size="lg" title={title} data-testid="ws-connect-journey">
        {step !== "done" ? <Steps steps={steps.filter((s) => s !== "agent" || !agent)} current={step} /> : null}
        {body}
      </DialogContent>
    </Dialog>
  );
}

/** The first step after the app is chosen. */
function stepAfterApp(shape: JourneyShape): StepId {
  const steps = stepsOf({ ...shape, appGiven: false });
  return steps[1] ?? "done";
}
