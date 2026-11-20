"use client";

import Link from "next/link";
import { useSearchParams } from "next/navigation";
import { useQueryClient } from "@tanstack/react-query";
import { useEffect, useState, type ReactNode } from "react";

import { ConnectorAvailableCallout } from "@/components/connectors/available-callout";
import { LlmKeyForm, useFooterSlot } from "@/components/credentials/llm-form";
import { ErrorState, Loading } from "@/components/states";
import { Button, buttonVariants } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { Dialog, DialogContent } from "@/components/ui/dialog";
import { CheckIcon } from "@/components/ui/icons";
import { Meter } from "@/components/ui/meter";
import { ErrorNotice, Notice } from "@/components/ui/notice";
import { PageHeader } from "@/components/ui/page-header";
import { StatusLine, type StatusTone } from "@/components/ui/status-line";
import { Tabs, tabId, tabPanelId } from "@/components/ui/tabs";
import { onboardingKeys, useAdminSetup, type SetupCheck, type SetupStatus, type SetupStep } from "@/lib/api/onboarding";
import { formatBytes } from "@/lib/api/attachments";
import { useAdminStorage } from "@/lib/api/org";
import { usePlugins } from "@/lib/api/plugins";
import { useBackfillStarterGrants, usePlacement, useSavePlacement, type PlacementView as Placement, type PutPlacement } from "@/lib/api/placement";
import { useAgents, useMe } from "@/lib/api/queries";
import { useSecrets } from "@/lib/api/secrets";
import { useSettingsSection } from "@/lib/api/settings";
import { formatTime } from "@/lib/format-date";
import { When } from "@/components/ui/when";
import { presetWords } from "@/components/policy/policy-presets";
import { detectPreset } from "@/lib/policy-presets";
import { helloMessage } from "@/lib/onboarding/guide";
import { adminRoutes, wsRoutes } from "@/lib/routes";
import { cn } from "@/lib/utils";

import { AttentionSection } from "./attention-section";
import { OwnerlessBanner } from "./ownerless-banner";
import { SignInGaps, gapsFromChecks } from "./sign-in-gaps";
import { PreviewsLine } from "./previews-line";
import { PushLine } from "./push-line";
import { SshLine } from "./ssh-line";

/** How many steps there are (D-391): builtin apps arrive approved, so there is no approval step. */
export const SETUP_STEPS = 5;

/** Who the key was just given to: every agent already in the organization gets it with the key (D-414). */
interface KeyGiven {
  agents: number;
  names: string[];
}

/** "Given to Daneel and 1 other agent already in Acme." */
export function givenToLine(g: KeyGiven, orgName: string): string {
  const first = g.names[0] ?? "an agent";
  const others = g.agents - (g.names[0] ? 1 : 0);
  return `Given to ${first}${others > 0 ? ` and ${others} other ${others === 1 ? "agent" : "agents"}` : ""} already in ${orgName}.`;
}

/** The toast after a key is added: what happened, and how many people signed in before the key (the backfill offer). */
interface Toast {
  text: string;
  waiting: number;
}

type StepId = "signin" | "placement" | "key" | "try" | "email";

interface Row {
  id: StepId;
  title: string;
  optional?: boolean;
  done: boolean;
  /** One line on the current state. */
  summary: ReactNode;
}

/** The step of the server's status by id. */
const stepOf = (setup: SetupStatus, id: string): SetupStep | undefined => setup.steps.find((s) => s.id === id);

/** The count of a step's checks by id, for the health lines. */
const checkOf = (setup: SetupStatus, stepId: string, id: string): SetupCheck | undefined => stepOf(setup, stepId)?.checks.find((c) => c.id === id);

/**
 * Sign-in is done when a provider exists and the admin signed in with it (`you`), or the roles step has no open item.
 * A missing role mapping is a warning for the health view, not an open step (P5-POLISH-11).
 */
const signInDone = (setup: SetupStatus) => !!setup.you || stepOf(setup, "roles")?.status === "ok";

/** An organization AI key exists: the `org_key` check, else the whole LLM step being fine. */
function keyDone(setup: SetupStatus): boolean {
  const llm = stepOf(setup, "llm");
  const key = llm?.checks.find((c) => c.id === "org_key");
  return key ? key.status === "ok" : llm?.status === "ok";
}

/** E-mail is done when SMTP works: the optional step carries no `email_not_configured` check. */
const emailDone = (setup: SetupStatus) => !(stepOf(setup, "optional")?.checks.some((c) => c.reason === "email_not_configured") ?? false);

/** The required steps (everything but the optional e-mail): setup is complete when they are all done. */
export function setupComplete(setup: SetupStatus, replied: boolean): boolean {
  return setup.placement_saved && signInDone(setup) && keyDone(setup) && replied;
}

/** How many of the five steps are done. */
export function doneCount(setup: SetupStatus, replied: boolean): number {
  return [signInDone(setup), setup.placement_saved && stepOf(setup, "placement")?.status !== "todo", keyDone(setup), replied, emailDone(setup)].filter(Boolean).length;
}

/**
 * (P5-REFINE-18 #14, C-1063) Try it, from the server: it ticks itself on the first finished turn of any agent the admin owns
 * (`try_it.done`) and is offered only when the agent can answer (`try_it.available`, false until a starter key exists).
 * Open chat goes straight to that agent's chat with the hello typed and not sent (C-1035's `?draft=`).
 */
function useTryIt(server: SetupStatus["try_it"] | undefined, hasKey: boolean) {
  const me = useMe();
  const workspaceId = server?.workspace_id ?? null;
  let base: { org: string; ws: string } | null = null;
  for (const m of me.data?.memberships ?? []) {
    const w = m.workspaces.find((x) => x.id === workspaceId);
    if (w) {
      base = { org: m.org.slug, ws: w.slug };
      break;
    }
  }
  const agents = useAgents(workspaceId ?? undefined);
  const agent = agents.data?.items.find((a) => a.id === server?.agent_id);
  const href =
    base && agent
      ? wsRoutes(base.org, base.ws).agent(agent.handle, { tab: "chat", draft: helloMessage(agent.name) })
      : base
        ? wsRoutes(base.org, base.ws).home()
        : "/";
  return {
    href,
    replied: server?.done === true,
    available: server ? server.available : hasKey,
    agentName: agent?.name ?? null,
    person: me.data?.user.display_name ?? null,
    ready: !me.isPending,
  };
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

/** (C-922) People who signed in before the starter grant existed: one click gives them the key too. Idempotent. */
function BackfillOffer({ waiting, afterKey = false }: { waiting: number; afterKey?: boolean }) {
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
        {afterKey
          ? `${waiting === 1 ? "1 person signed in" : `${waiting} people signed in`} before the key.`
          : waiting === 1
            ? "1 person signed in before and their agent can't use the key yet."
            : `${waiting} people signed in before and their agents can't use the key yet.`}
      </p>
      <Button variant="outline" size="sm" disabled={backfill.isPending} onClick={() => backfill.mutate()}>
        {afterKey ? `Give their ${waiting === 1 ? "agent" : "agents"} the key too` : `Also give it to the ${waiting} ${waiting === 1 ? "person" : "people"} who signed in before`}
      </Button>
      <ErrorNotice error={backfill.error} />
    </div>
  );
}

/**
 * The AI-key dialog of the checklist: the organization's key form for the default organization. The first key is a
 * starter grant by default, whenever placement allows one (C-922).
 */
function AiKeyDialog({
  open,
  onOpenChange,
  onSaved,
}: {
  open: boolean;
  onOpenChange: (o: boolean) => void;
  /** The key's name and, when the placement gave it to agents already there (C-1061), who got it. */
  onSaved: (keyName: string, applied: KeyGiven | null) => void;
}) {
  const placement = usePlacement();
  const savePlacement = useSavePlacement();
  const org = placement.data?.default_org ?? null;
  const { footer, target } = useFooterSlot();
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent size="lg" title="Add an organization AI key" description={org ? `Every new agent in ${org.name} gets this key.` : undefined} footer={footer}>
        {org ? (
          <LlmKeyForm
            scope={{ kind: "organization", id: org.id }}
            orgId={org.id}
            submitLabel="Add key"
            onCancel={() => onOpenChange(false)}
            footerTarget={target}
            onSaved={(secret) => {
              const p = placement.data;
              if (p && startersAllowed(p)) {
                const others = p.starter_grants.map((g) => ({ kind: g.kind, id: g.id }));
                savePlacement.mutate(putWithStarters(p, [...others, { kind: "secret", id: secret.id }]), {
                  onSuccess: (v) => {
                    const mine = (v.starter_applied ?? []).find((a) => a.id === secret.id);
                    onSaved(secret.name, mine && mine.agents_granted > 0 ? { agents: mine.agents_granted, names: mine.agent_names } : null);
                  },
                  onError: () => onSaved(secret.name, null),
                });
              } else onSaved(secret.name, null);
            }}
          />
        ) : (
          <Loading />
        )}
      </DialogContent>
    </Dialog>
  );
}

/**
 * (P5-REFINE-18 #2, D-410) Under the checklist, not counted: what agents may do right now and where to change it.
 * The preset is read from the governance section's effective values, as on the card itself.
 */
function PresetLine() {
  const q = useSettingsSection("governance");
  if (!q.data) return null;
  return (
    <p className="text-sm text-ink-2" data-testid="preset-line">
      What agents may do · <strong className="font-semibold text-ink">{presetWords(detectPreset(q.data.settings))}</strong> ·{" "}
      <Link href={`${adminRoutes.internetAccess()}#presets`} className="font-medium text-ink underline underline-offset-2">
        Change
      </Link>
    </p>
  );
}

const rowButton = cn(buttonVariants({ variant: "outline", size: "sm" }));

/** One row of the checklist: a status circle (a check when done, the number otherwise), a title, one line, one button. */
function ChecklistRow({ n, row, action, primary }: { n: number; row: Row; action: ReactNode; primary: boolean }) {
  return (
    <li id={`setup-${row.id}`} data-step={row.id} data-done={row.done} className={cn("flex flex-wrap items-center gap-3 px-4 py-3 sm:px-5", primary && "bg-pending-panel")}>
      <span
        aria-hidden="true"
        className={cn(
          "inline-flex size-7 shrink-0 items-center justify-center rounded-full text-sm font-semibold",
          row.done ? "bg-accent-soft text-accent-ink" : primary ? "bg-pending-panel text-pending-ink" : "bg-subtle text-ink-2",
        )}
      >
        {row.done ? <CheckIcon className="size-4" /> : n}
      </span>
      <div className="min-w-0 flex-1">
        <h2 className="font-display text-base font-semibold text-ink">
          {row.title}
          {row.optional ? <span className="ml-1.5 text-sm font-normal text-muted">· optional</span> : null}
          <span className="sr-only">{row.done ? " (done)" : " (to do)"}</span>
        </h2>
        <p className="text-sm text-ink-2">{row.summary}</p>
      </div>
      <div className="shrink-0">{action}</div>
    </li>
  );
}

/** The checklist: five rows, each one state and one button. */
function Checklist({
  setup,
  tryIt,
  toast,
  given,
  onKeyAdded,
  onDismissToast,
}: {
  setup: SetupStatus;
  tryIt: ReturnType<typeof useTryIt>;
  toast: Toast | null;
  given: KeyGiven | null;
  onKeyAdded: (keyName: string, applied: KeyGiven | null) => void;
  onDismissToast: () => void;
}) {
  const placement = usePlacement();
  const org = placement.data?.default_org ?? null;
  // (P5-POLISH-11) No request before the placement names an organization (it was GET /v1/orgs//secrets → 400).
  const secrets = useSecrets({ kind: "organization", id: org?.id ?? "" }, !!org);
  const orgKey = (secrets.data?.items ?? []).find((s) => s.kind === "llm" && s.status === "active");
  const [keyOpen, setKeyOpen] = useState(false);
  const uploaded = usePlugins("pending_approval");
  const waiting = (uploaded.data?.items ?? []).filter((p) => p.origin !== "aurora").length;
  const you = setup.you;
  const unserved = setup.personal_workspaces_without_starter ?? 0;
  const placed = setup.placement_saved && stepOf(setup, "placement")?.status !== "todo";

  const rows: Row[] = [
    { id: "signin", title: "Sign-in", done: signInDone(setup), summary: you ? (you.provider_name ?? you.provider) : "Development sign-in" },
    {
      id: "placement",
      title: "Where new people land",
      done: placed,
      summary:
        placed && org
          ? `The ${org.name} organization, ${placement.data?.personal_workspace.enabled ? "each in their own workspace" : "in a shared workspace"}`
          : "Choose the organization and workspace a new person gets on day one",
    },
    {
      id: "key",
      title: "AI key",
      done: keyDone(setup),
      summary: keyDone(setup) ? (
        <>
          {`${orgKey?.name ?? "The organization's key"} · every new agent${org ? ` in ${org.name}` : ""} gets it`}
          {given && org ? (
            <span className="mt-1 block" data-testid="key-given">
              {givenToLine(given, org.name)}
            </span>
          ) : null}
          {unserved > 0 ? (
            <span className="mt-2 block" data-testid="key-backfill">
              <BackfillOffer waiting={unserved} afterKey />
            </span>
          ) : null}
        </>
      ) : org ? "Add an organization AI key" : "Choose where new people land first",
    },
    {
      id: "try",
      title: "Try it",
      done: tryIt.replied,
      // No starter key (D-414): Try it is not offered until an agent can answer.
      summary: tryIt.replied
        ? `${tryIt.agentName ?? "Your agent"} answered ${tryIt.person ?? "you"}`
        : tryIt.available
          ? "Say hello to your own agent"
          : "Comes after the AI key",
    },
    {
      id: "email",
      title: "E-mail",
      optional: true,
      done: emailDone(setup),
      summary: emailDone(setup) ? "Invitations and approval notices are sent" : "For invitations and approval notices",
    },
  ];
  const first = rows.findIndex((r) => !r.done && !r.optional);
  // The current step's button is filled, the others are outline (`after/e4.png`).
  // A done row's button is plain text (Review, Change).
  const linkButton = (i: number) =>
    i === first ? cn(buttonVariants({ variant: "primary", size: "sm" }), "text-on-primary!") : rows[i]!.done ? cn(buttonVariants({ variant: "ghost", size: "sm" })) : rowButton;
  const actions: Record<StepId, ReactNode> = {
    signin: (
      <Link href={adminRoutes.signIn()} className={linkButton(0)}>
        Review
      </Link>
    ),
    placement: (
      <Link href={adminRoutes.people("new-users")} className={linkButton(1)}>
        {placed ? "Change" : "Choose"}
      </Link>
    ),
    key: rows[2]!.done ? null : (
      <Button variant={first === 2 ? "primary" : "outline"} size="sm" disabled={!org || !placement.data?.saved} onClick={() => setKeyOpen(true)}>
        Add key
      </Button>
    ),
    try: tryIt.available ? (
      <Link href={tryIt.href} className={linkButton(3)}>
        Open chat
      </Link>
    ) : null,
    email: (
      <Link href={adminRoutes.platform("email")} className={rows[4]!.done ? cn(buttonVariants({ variant: "ghost", size: "sm" })) : rowButton}>
        {rows[4]!.done ? "Review" : "Set up"}
      </Link>
    ),
  };
  const done = rows.filter((r) => r.done).length;
  return (
    <div className="space-y-4">
      {toast ? (
        <Notice tone="accent" role="status" className="flex items-center justify-between gap-3" data-testid="setup-toast">
          <span className="space-y-2">
            <span className="block">{toast.text}</span>
            {toast.waiting > 0 ? <BackfillOffer waiting={toast.waiting} afterKey /> : null}
          </span>
          <button type="button" className="text-sm font-medium underline" onClick={onDismissToast}>
            Dismiss
          </button>
        </Notice>
      ) : null}
      <div className="flex items-center justify-between gap-3 text-sm text-ink-2">
        <span>Five steps from a fresh install to your first answer.</span>
        <span id="setup-progress-label" className="whitespace-nowrap">{`${done} of ${SETUP_STEPS}`}</span>
      </div>
      <Meter ratio={done / SETUP_STEPS} role="progressbar" aria-label="Setup progress" aria-valuemin={0} aria-valuemax={SETUP_STEPS} aria-valuenow={done} />
      <Card className="overflow-hidden p-0 sm:p-0">
        <ol aria-label="Setup steps" className="divide-y divide-line">
          {rows.map((r, i) => (
            <ChecklistRow key={r.id} n={i + 1} row={r} action={actions[r.id]} primary={i === first} />
          ))}
        </ol>
      </Card>
      <PresetLine />
      {waiting > 0 ? (
        <p className="text-sm text-ink-2" data-testid="uploaded-waiting">
          {waiting === 1 ? "1 uploaded app is waiting for review. " : `${waiting} uploaded apps are waiting for review. `}
          <Link href={adminRoutes.apps("apps")} className="font-medium text-ink underline underline-offset-2">
            Open Apps
          </Link>
        </p>
      ) : null}
      <AiKeyDialog
        open={keyOpen}
        onOpenChange={setKeyOpen}
        onSaved={(keyName, applied) => {
          setKeyOpen(false);
          onKeyAdded(keyName, applied);
        }}
      />
    </div>
  );
}

const tone = (s?: SetupCheck["status"]): StatusTone => (s === "ok" ? "ok" : s === "warning" ? "attention" : "idle");

/** "1 runner online." / "3 runners online.": the server's "runner(s)" is turned into the right number. */
const pluralRunners = (msg: string, n: string | null | undefined) => {
  const count = Number(n);
  return msg.replace(/(\d+) runner\(s\)/, (_m, d: string) => `${d} ${Number(d) === 1 ? "runner" : "runners"}`).replace(/\brunner\(s\)/, count === 1 ? "runner" : "runners");
};

function HealthRow({ tone: t, title, children, href, hrefLabel }: { tone: StatusTone; title: string; children: ReactNode; href?: string; hrefLabel?: string }) {
  return (
    <li className="px-4 py-3" data-health={title}>
      <StatusLine
        tone={t}
        action={
          href ? (
            <Link href={href} className="text-sm font-medium text-ink underline underline-offset-2">
              {hrefLabel}
            </Link>
          ) : undefined
        }
      >
        <span className="font-semibold text-ink">{title}</span> · <span>{children}</span>
      </StatusLine>
    </li>
  );
}

/** The terminal section is available on this deployment (the same test as the Computers page). */
function availableOf(section: { extra?: unknown }): boolean {
  return (section.extra as { available?: boolean } | null | undefined)?.available === true;
}
/** Is the terminal switched on in the section: the saved value, else its default; `null` when the key is absent. */
function terminalOn(section: { settings: { key: string; value: unknown; default?: unknown }[] }): boolean | null {
  const s = section.settings.find((x) => x.key === "terminal.enabled");
  if (!s) return null;
  return (s.value ?? s.default) === true;
}

/**
 * (P5-REFINE-18 #15, D-383) One endpoint in the AI models line, by its display name and only what a real call proved:
 * "Company AI gateway answered 2 min ago." or "Users AI gateway has not been asked yet." A failed real call says when.
 */
function EndpointLine({ c }: { c: SetupCheck }) {
  const name = c.message.replace(/ (answers|did not answer)\.$/, "");
  const ok = c.last_ok_at ?? null;
  const err = c.last_error_at ?? null;
  const failedLast = err !== null && (ok === null || err > ok);
  return (
    <span className="mr-1">
      <strong className="font-semibold text-ink">{name}</strong>{" "}
      {failedLast ? (
        <>did not answer at {formatTime(err)}.</>
      ) : ok !== null && c.evidence === "call" ? (
        <>
          answered <When value={ok} />.
        </>
      ) : c.status !== "ok" ? (
        <>did not answer.</>
      ) : (
        <>has not been asked yet.</>
      )}
    </span>
  );
}

/** After setup: what is healthy and what needs the admin, from the same status as the checklist (`14 §9`). */
function HealthView({ setup, onRunSetup }: { setup: SetupStatus; onRunSetup: () => void }) {
  const placement = usePlacement();
  const storage = useAdminStorage();
  const terminal = useSettingsSection("terminal");
  const optional = stepOf(setup, "optional");
  const gaps = gapsFromChecks(optional?.checks ?? []);
  const blocked = gaps.filter((g) => !g.tokenOnly);
  const noMail = !emailDone(setup);
  const orgSlug = placement.data?.default_org?.slug;
  const roles = stepOf(setup, "roles");
  const llm = stepOf(setup, "llm");
  const endpoints = llm?.checks.filter((c) => c.id.startsWith("endpoint:")) ?? [];
  const down = endpoints.filter((c) => c.status !== "ok");
  const gateway = checkOf(setup, "runners", "gateway");
  const runner = checkOf(setup, "runners", "runners");
  const needs = blocked.length + (noMail ? 1 : 0);
  const withStarter = (placement.data?.starter_grants.length ?? 0) > 0;
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
        <section aria-labelledby="needs-you-h" className="space-y-4 rounded-card border border-line bg-surface p-4">
          <div>
            <h2 id="needs-you-h" className="font-display text-base font-semibold text-ink">
              Needs you
            </h2>
            <p className="text-sm text-ink-2">Each item says what is missing and opens the page that fixes it.</p>
          </div>
          <SignInGaps gaps={blocked.length > 0 ? gaps : blocked} where={{ level: "admin", orgSlug }} />
          {noMail ? (
            <div data-reason="email_not_configured">
              <StatusLine
                tone="attention"
                action={
                  <Link href={adminRoutes.platform("email")} className={rowButton}>
                    Set up e-mail
                  </Link>
                }
              >
                E-mail isn&apos;t set up. Aurora can&apos;t send approval or invitation e-mails.
              </StatusLine>
            </div>
          ) : null}
        </section>
      ) : null}
      {blocked.length === 0 && gaps.some((g) => g.tokenOnly) ? <SignInGaps gaps={gaps.filter((g) => g.tokenOnly)} where={{ level: "admin", orgSlug }} /> : null}
      <Card className="p-0">
        <h2 className="px-4 pt-4 font-display text-base font-semibold text-ink">Health</h2>
        <ul aria-label="Health" className="divide-y divide-line">
          {roles && roles.status !== "ok" ? (
            <HealthRow tone="attention" title="Sign-in" href={adminRoutes.signIn()} hrefLabel="Open sign-in">
              {roles.checks.find((c) => c.status !== "ok")?.message ?? "Role mapping needs a look."}
            </HealthRow>
          ) : null}
          <HealthRow tone={tone(runner?.status)} title="Runners" href={adminRoutes.platform("runners")} hrefLabel="Open runners">
            {runner?.message ? pluralRunners(runner.message, runner.value) : (stepOf(setup, "runners")?.reason ?? "No runner is registered.")}
          </HealthRow>
          <HealthRow tone={tone(gateway?.status)} title="Gateway">
            {gateway?.message ?? "No gateway has been seen."}
          </HealthRow>
          <HealthRow
            tone={endpoints.length === 0 ? "idle" : down.length > 0 ? "attention" : "ok"}
            title="AI models"
            href={adminRoutes.aiModels("endpoints")}
            hrefLabel="Open AI models"
          >
            {endpoints.length === 0 ? (
              "No AI endpoint is set up."
            ) : (
              endpoints.map((c) => <EndpointLine key={c.id} c={c} />)
            )}
          </HealthRow>
          {terminal.data && availableOf(terminal.data) && terminalOn(terminal.data) === false ? (
            <HealthRow tone="idle" title="Web terminal" href={adminRoutes.sectionPage("terminal", "terminal.enabled")} hrefLabel="Turn on">
              Off. People see “not available” on their agent&apos;s Computer tab.
            </HealthRow>
          ) : null}
          <HealthRow tone="ok" title="New people" href={adminRoutes.people("new-users")} hrefLabel="Change">
            {setup.placement_saved ? "They land where you chose, with a personal workspace and the organization's AI key." : "Not saved yet."}
          </HealthRow>
          {storage.data ? (
            <HealthRow tone="ok" title="Storage" href={adminRoutes.data()} hrefLabel="Open Data">
              {formatBytes(storage.data.blob_bytes)} in {storage.data.blob_count} stored files.
            </HealthRow>
          ) : null}
        </ul>
      </Card>
      {withStarter ? <BackfillOffer waiting={setup.personal_workspaces_without_starter ?? 0} /> : null}
      <div className="space-y-3">
        <PreviewsLine />
        <SshLine />
        <PushLine />
      </div>
    </div>
  );
}

type View = "checklist" | "health";

/**
 * Administration › Overview (`14 §Agents, accounts and access` §9): the five-step checklist until the required steps are
 * done, then the health view from the same status. A **Health** tab is always there, **Run setup again** brings the
 * checklist back. Saving a step's work (the AI key) stays on the checklist with a toast (D-391).
 */
export function SetupView() {
  const qc = useQueryClient();
  const setup = useAdminSetup();
  const params = useSearchParams();
  const tryIt = useTryIt(setup.data?.try_it, setup.data ? keyDone(setup.data) : false);
  // `null` = automatic: the health view when the required steps are done. A choice (or a step just finished) pins it.
  const [view, setView] = useState<View | null>(params.get("setup") === "1" ? "checklist" : null);
  const [toast, setToast] = useState<Toast | null>(null);
  useEffect(() => {
    if (!toast || toast.waiting > 0) return;
    const t = window.setTimeout(() => setToast(null), 8000);
    return () => window.clearTimeout(t);
  }, [toast]);

  const complete = setup.data ? setupComplete(setup.data, tryIt.replied) : false;
  const shown: View = view ?? (complete && tryIt.ready ? "health" : "checklist");

  const [given, setGiven] = useState<KeyGiven | null>(null);
  async function keyAdded(keyName: string, applied: KeyGiven | null) {
    setView("checklist");
    setGiven(applied);
    await qc.invalidateQueries({ queryKey: onboardingKeys.setup });
    const fresh = qc.getQueryData<SetupStatus>(onboardingKeys.setup);
    // (P5-REFINE-18 #14, D-414) "Company key added. Daneel can use it now." The agents already there got it with the key.
    const who = applied?.names[0];
    const lead = who ? `${keyName} added. ${who} can use it now.` : `${keyName} added.`;
    setToast({ text: fresh ? `${lead} ${doneCount(fresh, tryIt.replied)} of ${SETUP_STEPS} done.` : lead, waiting: fresh?.personal_workspaces_without_starter ?? 0 });
  }

  return (
    <div className="max-w-page space-y-6">
      <PageHeader
        title={shown === "health" ? "Overview" : "Set up Aurora"}
        description={shown === "health" ? "What needs you, and whether Aurora is healthy." : undefined}
        className="mb-0"
        actions={
          <Tabs
            label="Overview view"
            idBase="overview"
            value={shown}
            onValueChange={(v) => setView(v as View)}
            items={[
              { value: "checklist", label: "Checklist" },
              { value: "health", label: "Health" },
            ]}
          />
        }
      />
      {setup.isPending ? <Loading /> : null}
      {setup.isError ? <ErrorState error={setup.error} onRetry={() => setup.refetch()} /> : null}
      <AttentionSection />
      <OwnerlessBanner />
      <ConnectorAvailableCallout />
      <div role="tabpanel" id={tabPanelId("overview", shown)} aria-labelledby={tabId("overview", shown)} className="space-y-6">
        {setup.data && shown === "health" ? <HealthView setup={setup.data} onRunSetup={() => setView("checklist")} /> : null}
        {setup.data && shown === "checklist" ? (
          <Checklist setup={setup.data} tryIt={tryIt} toast={toast} given={given} onKeyAdded={(n, a) => void keyAdded(n, a)} onDismissToast={() => setToast(null)} />
        ) : null}
      </div>
    </div>
  );
}
