"use client";

import { useState, type ReactNode } from "react";

import { OrgGiveAccessDialog } from "@/components/accounts/org-give-access";
import { ErrorState, Loading } from "@/components/states";
import { Button } from "@/components/ui/button";
import { ChoiceCards } from "@/components/ui/choice-cards";
import { Dialog, DialogContent } from "@/components/ui/dialog";
import { LEVEL_LABEL, LEVEL_ORDER } from "@/lib/accounts";
import { accountKeys, useAccounts, type Level } from "@/lib/api/accounts";
import { appDisplayName } from "@/lib/api/app-name";
import { useApp, useConnectPlan } from "@/lib/api/apps";
import { useOrg, useWorkspaces } from "@/lib/api/queries";
import { cn } from "@/lib/utils";
import { useQueryClient } from "@tanstack/react-query";

import { AppStep, type AppChoice } from "./app-step";
import { levelsForApp, ORG_STEP_LABEL, orgDoneSentence, orgStepsOf, type OrgStepId } from "./logic";
import { SignInStep } from "./sign-in-step";

/** What each limit means, in the words of the access choices (`14 §2`, the limit). */
const LIMIT_TEXT: Record<Level, string> = {
  read: "No agent can change anything with this account.",
  ask: "Agents ask you before each change; none can change anything without asking.",
  act: "Agents may be given any choice, up to changing without asking.",
};

/**
 * **Connect for the whole organization** (C-936, D-332, D-353): the connect journey in its organization mode. No Agent
 * step; the Account step offers an existing organization account of the app ("Use <name>"), which skips to Done; the
 * Access step sets the account's **limit**, "Agents may at most:", over the same three choices with Read only
 * preselected; Done says no agent can use it yet and offers **Give access** (the picker over agents of the workspaces
 * the person owns) and **Done**. Organization owners and admins only.
 */
export function OrgConnectJourney({ orgId, appId, onClose }: { orgId: string; appId?: string | null; onClose: () => void }) {
  const qc = useQueryClient();
  const org = useOrg(orgId);
  const workspaces = useWorkspaces(orgId);
  // The connect plan (which ways to connect work) is read in a workspace the person owns; the account is not made there.
  const planWs = (workspaces.data?.items ?? []).find((w) => w.my_role === "owner")?.id;
  const [choice, setChoice] = useState<AppChoice | null>(appId ? { kind: "app", appId } : null);
  const [step, setStep] = useState<OrgStepId | null>(null);
  const [accountPick, setAccountPick] = useState<string | null>(null);
  const [limit, setLimit] = useState<Level>("read");
  const [done, setDone] = useState<{ accountId: string; appName: string; customOnly: boolean } | null>(null);
  const [giving, setGiving] = useState(false);

  const realApp = choice?.kind === "app" ? choice.appId : undefined;
  const detail = useApp(realApp, planWs);
  const plan = useConnectPlan(realApp, planWs);
  const accounts = useAccounts({ kind: "organization", id: orgId });
  const app = detail.data;
  const existing = (accounts.data?.items ?? []).filter((a) => a.kind === "app" && a.app?.app_id === realApp && a.can.manage);
  const account = accountPick ?? existing[0]?.ref.id ?? "new";
  const useExisting = account !== "new" && existing.some((a) => a.ref.id === account);
  const choices = app ? levelsForApp(app) : [];
  const shape = { appGiven: !!appId, hasExisting: existing.length > 0, useExisting, hasChoice: choices.length > 0 };
  const steps = orgStepsOf(shape);
  const ready = !!choice && !!app && !accounts.isPending && !!planWs;
  const current: OrgStepId = !choice ? "app" : (step ?? (ready ? (steps[appId ? 0 : 1] ?? "done") : "app"));
  const orgName = org.data?.name ?? "your organization";
  const appName = app ? appDisplayName(app) : "";
  const givenAccount = done ? (accounts.data?.items ?? []).find((a) => a.ref.id === done.accountId) : undefined;
  const effective: Level = choices.includes(limit) ? limit : (choices[0] ?? "read");

  const next = (from: OrgStepId) => setStep(steps[Math.min(steps.indexOf(from) + 1, steps.length - 1)] ?? "done");
  const back = (from: OrgStepId) => {
    const i = steps.indexOf(from);
    if (i > 0) setStep(steps[i - 1]!);
  };
  const refresh = () => {
    void qc.invalidateQueries({ queryKey: accountKeys.all });
    void qc.invalidateQueries({ queryKey: ["connections"] });
    void qc.invalidateQueries({ queryKey: ["apps"] });
  };

  let body: ReactNode;
  if (workspaces.isError) body = <ErrorState error={workspaces.error} onRetry={() => void workspaces.refetch()} />;
  else if (choice?.kind === "app" && detail.isError) body = <ErrorState error={detail.error} onRetry={() => void detail.refetch()} />;
  else if (workspaces.isSuccess && !planWs) body = <p className="text-sm text-ink-2">You do not own a workspace in this organization yet. Create one first.</p>;
  else if (current === "app" && !appId) {
    body = (
      <AppStep
        workspaceId={planWs ?? ""}
        orgId={orgId}
        onPick={(c) => {
          setChoice(c);
          setAccountPick(null);
          setStep(null);
        }}
      />
    );
  } else if (!ready) body = <Loading />;
  else if (current === "account") {
    body = (
      <div className="space-y-3">
        <ChoiceCards<string>
          legend={`Which ${appName} account?`}
          value={useExisting ? account : "new"}
          onChange={setAccountPick}
          options={[
            ...existing.map((a, i) => ({ value: a.ref.id, label: `Use ${a.name}`, badge: i === 0 ? "Recommended" : undefined })),
            { value: "new", label: "Connect another account" },
          ]}
        />
        <Footer
          onBack={steps.indexOf("account") > 0 ? () => back("account") : undefined}
          onNext={() => {
            if (useExisting) {
              setDone({ accountId: account, appName, customOnly: app?.app_restrictions?.custom_only === true });
              setStep("done");
            } else next("account");
          }}
        />
      </div>
    );
  } else if (current === "access") {
    body = (
      <div className="space-y-3">
        <ChoiceCards<Level>
          legend="Agents may at most:"
          value={effective}
          onChange={setLimit}
          options={LEVEL_ORDER.filter((l) => choices.includes(l)).map((l) => ({
            value: l,
            label: LEVEL_LABEL[l],
            description: LIMIT_TEXT[l],
            badge: l === "read" ? "Recommended" : undefined,
          }))}
        />
        <p className="text-xs text-muted">You can change this at any time on the organization&apos;s Accounts tab.</p>
        <Footer onBack={steps.indexOf("access") > 0 ? () => back("access") : undefined} onNext={() => next("access")} />
      </div>
    );
  } else if (current === "signin" && app) {
    body = plan.isPending ? (
      <Loading />
    ) : plan.isError ? (
      <ErrorState error={plan.error} onRetry={() => void plan.refetch()} />
    ) : (
      <SignInStep
        key={app.app_id}
        app={app}
        plan={plan.data}
        workspaceId={planWs ?? ""}
        orgId={orgId}
        orgMode
        level={choices.length > 0 ? effective : null}
        agentIds={[]}
        fixedInstanceId={choice?.kind === "app" ? choice.instanceId : undefined}
        hasWriteTools={app.tools.some((t) => t.group === "write")}
        onBack={steps.indexOf("signin") > 0 ? () => back("signin") : undefined}
        onResult={(r) => {
          refresh();
          setDone({ accountId: r.accountId, appName, customOnly: app.app_restrictions?.custom_only === true });
          setStep("done");
        }}
      />
    );
  } else if (current === "done" && done) {
    body = (
      <div role="status" className="space-y-3 text-sm">
        <p className="text-base font-medium text-ink">{orgDoneSentence(done.appName, orgName)}</p>
        <div className="flex flex-wrap gap-2 pt-1">
          <Button size="sm" onClick={() => setGiving(true)}>
            Give access
          </Button>
          <Button size="sm" variant="ghost" onClick={onClose}>
            Done
          </Button>
        </div>
        {giving && givenAccount ? (
          <OrgGiveAccessDialog
            account={givenAccount}
            orgId={orgId}
            orgName={orgName}
            onOpenChange={(o) => {
              setGiving(o);
              if (!o) refresh();
            }}
            subject={{
              target: { kind: "connection", id: done.accountId },
              scope: { kind: "organization", id: orgId },
              orgId,
              appId: realApp,
              customOnly: done.customOnly,
            }}
          />
        ) : null}
      </div>
    );
  } else body = <Loading />;

  const title = appName ? `Connect ${appName} for ${orgName}` : `Connect an app or website for ${orgName}`;
  return (
    <Dialog open onOpenChange={(o) => (o ? null : onClose())}>
      <DialogContent
        size="lg"
        title={title}
        data-testid="org-connect-journey"
        header={
          current !== "done" ? (
            <ol className="flex gap-1.5 text-xs font-semibold uppercase tracking-wider" aria-label="Steps">
              {steps.map((s) => (
                <li key={s} aria-current={s === current ? "step" : undefined} className={cn("flex-1 border-t-4 pt-1.5", s === current ? "border-accent text-ink" : "border-line text-muted")}>
                  {ORG_STEP_LABEL[s]}
                </li>
              ))}
            </ol>
          ) : null
        }>
        {body}
      </DialogContent>
    </Dialog>
  );
}

function Footer({ onBack, onNext }: { onBack?: () => void; onNext: () => void }) {
  return (
    <div className="flex flex-wrap justify-end gap-2 pt-2">
      {onBack ? (
        <Button type="button" variant="ghost" onClick={onBack}>
          Back
        </Button>
      ) : null}
      <Button type="button" size="lg" onClick={onNext}>
        Next
      </Button>
    </div>
  );
}
