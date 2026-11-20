"use client";

import Link from "next/link";
import type { ReactNode } from "react";

import { ClientRow } from "@/components/apps/oauth-clients-panel";
import { ErrorState, Loading } from "@/components/states";
import { Altitude } from "@/components/ui/altitude";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { CopyValue } from "@/components/ui/copy-value";
import { Markdown } from "@/components/ui/markdown";
import { Notice } from "@/components/ui/notice";
import { WhoCan } from "@/components/ui/who-can";
import { appDisplayName } from "@/lib/api/app-name";
import { useAppGuide, useOAuthClients, type AppDetail, type AppSetup, type CredScope, type SetupScope } from "@/lib/api/apps";
import {
  CHIP_LABEL,
  clientLabel,
  methodChip,
  methodSentence,
  type SetupActions,
  type SetupChip,
  type SetupView,
} from "@/lib/apps/setup-copy";
import { orgRoutes } from "@/lib/routes";

const CHIP_TONE: Record<SetupChip, "accent" | "neutral"> = { ready: "accent", needs_setup: "neutral", admin_only: "neutral" };

export function StatusChip({ chip }: { chip: SetupChip }) {
  return <Badge tone={CHIP_TONE[chip]}>{CHIP_LABEL[chip]}</Badge>;
}

function Heading({ id, children }: { id: string; children: ReactNode }) {
  return (
    <h2 id={id} className="font-display text-lg font-semibold text-ink">
      {children}
    </h2>
  );
}

/** Status at the top: one sentence per way to connect, its chip, and what the viewer can do about it here. */
export function WaysToConnect({
  setup,
  view,
  orgName,
  orgSlug,
  packageId,
  actions,
  onRegisterClient,
}: {
  setup: AppSetup;
  view: SetupView;
  orgName: string;
  orgSlug: string;
  packageId: string;
  actions: SetupActions;
  onRegisterClient: () => void;
}) {
  const appName = appDisplayName(setup);
  return (
    <Card className="space-y-0 overflow-hidden p-0" data-testid="setup-ways">
      <div className="px-5 py-4">
        <Heading id="ways-h">Ways to connect</Heading>
      </div>
      {!setup.available ? (
        <div className="border-t border-line px-5 py-4">
          <Notice tone="pending" role="status">
            <p>{appName} is not available {view === "workspace" ? "in this workspace" : `in ${orgName}`}.</p>
            {setup.available_who_can ? <WhoCan value={setup.available_who_can} className="mt-1" /> : null}
          </Notice>
        </div>
      ) : null}
      <ul aria-labelledby="ways-h">
        {setup.methods.map((m) => {
          const chip = methodChip(m, view, setup.can);
          const needsClient = !m.ready && m.needs.includes("oauth_client");
          const needsInstance = !m.ready && m.needs.includes("instance");
          return (
            <li
              key={m.id}
              className={`flex flex-wrap items-start gap-x-4 gap-y-2 border-t border-line px-5 py-4 ${chip === "ready" ? "" : "bg-pending-panel"}`}
            >
              <span className="mt-0.5 w-28 shrink-0">
                <StatusChip chip={chip} />
              </span>
              <div className="min-w-0 flex-1 space-y-2">
                <p>
                  <span className="font-medium text-ink">{m.label}</span>
                  <br />
                  <span className="text-ink-2">{methodSentence(appName, m, view, { orgName, available: setup.available })}</span>
                </p>
                {needsClient && actions.registerClient ? (
                  <Button size="sm" onClick={onRegisterClient}>
                    Register a sign-in app for {view === "workspace" ? "this workspace" : orgName}
                  </Button>
                ) : null}
                {needsInstance && actions.registerInstance ? (
                  <Button asChild size="sm">
                    <Link href={orgRoutes(orgSlug).apps("self-hosted", { package: packageId })}>Register a self-hosted app</Link>
                  </Button>
                ) : null}
                {!m.ready && m.who_can ? (
                  <div className="space-y-0.5">
                    {needsClient && actions.registerClient && view === "workspace" ? (
                      <p className="text-xs text-muted">For everyone in {orgName}</p>
                    ) : null}
                    <WhoCan value={m.who_can} className="text-xs" />
                  </div>
                ) : null}
              </div>
            </li>
          );
        })}
      </ul>
    </Card>
  );
}

function SetUpRow({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="flex flex-wrap items-center gap-x-3 gap-y-1 border-t border-line py-3 first:border-t-0">
      <span className="min-w-0 flex-1 text-sm text-ink">{label}</span>
      <span className="min-w-0 text-sm text-muted">{children}</span>
    </div>
  );
}

/**
 * What is set up, and where: OAuth clients by level, the self-hosted apps registered for the app, availability. The
 * viewer's own level has its clients managed right here (Register, Edit, Remove).
 */
export function WhatIsSetUp({
  setup,
  detail,
  view,
  scope,
  orgName,
  orgSlug,
  actions,
  usesClients,
  onRegisterClient,
}: {
  setup: AppSetup;
  detail: AppDetail;
  view: SetupView;
  scope: CredScope & { kind: "workspace" | "organization" };
  orgName: string;
  orgSlug: string;
  actions: SetupActions;
  usesClients: boolean;
  onRegisterClient: () => void;
}) {
  const appName = appDisplayName(setup);
  const names = (refs: { used_by_methods: { label: string }[] }[] | null | undefined): string =>
    !refs || refs.length === 0 ? "None" : refs.map((c) => clientLabel(appName, c.used_by_methods)).join(", ");
  const showInstances = setup.instances.length > 0 || actions.registerInstance;
  return (
    <Card className="space-y-0 p-5" data-testid="setup-where">
      <h2 id="set-h" className="mb-1 font-display text-base font-semibold text-ink">
        What is set up, and where
      </h2>
      {usesClients ? (
        <>
          {view === "workspace" ? <SetUpRow label="Sign-in app · this workspace">{names(setup.clients.workspace)}</SetUpRow> : null}
          <SetUpRow label={`Sign-in app · ${orgName}`}>{names(setup.clients.organization)}</SetUpRow>
          <SetUpRow label="Sign-in app · Aurora">{setup.clients.instance.set_up ? "Set up" : "Not set up"}</SetUpRow>
        </>
      ) : null}
      {showInstances ? (
        <SetUpRow label="Self-hosted apps">
          {setup.instances.length === 0 ? (
            "None"
          ) : (
            <span className="space-y-0.5">
              {setup.instances.map((i) => (
                <span key={i.id} className="block break-all font-mono text-xs text-ink-2">
                  {i.label} · {i.base_url}
                </span>
              ))}
            </span>
          )}
          {actions.registerInstance ? (
            <span className="ml-2">
              <Link className="font-medium text-link underline" href={orgRoutes(orgSlug).apps("self-hosted", { package: detail.package_id })}>
                Register a self-hosted app
              </Link>
            </span>
          ) : null}
        </SetUpRow>
      ) : null}
      <SetUpRow label={`Available in ${orgName}`}>
        <Badge tone={setup.available ? "accent" : "neutral"}>{setup.available ? "Yes" : "No"}</Badge>
        {actions.setAvailability ? (
          <span className="ml-2">
            <Link className="font-medium text-link underline" href={orgRoutes(orgSlug).apps("apps", undefined, "availability")}>
              Change availability
            </Link>
          </span>
        ) : null}
      </SetUpRow>
      {usesClients && (actions.registerClient || ownLevelHasClients(setup, view)) ? (
        <OwnClients
          scope={scope}
          detail={detail}
          view={view}
          orgName={orgName}
          canRegister={actions.registerClient}
          onRegister={onRegisterClient}
        />
      ) : null}
    </Card>
  );
}

function ownLevelHasClients(setup: AppSetup, view: SetupView): boolean {
  return ((view === "workspace" ? setup.clients.workspace : setup.clients.organization) ?? []).length > 0;
}

/** The viewer's own level's clients of this app, with Register, Edit, Remove (the OAuth clients panel's row). */
function OwnClients({
  scope,
  detail,
  view,
  orgName,
  canRegister,
  onRegister,
}: {
  scope: CredScope & { kind: "workspace" | "organization" };
  detail: AppDetail;
  view: SetupView;
  orgName: string;
  canRegister: boolean;
  onRegister: () => void;
}) {
  const list = useOAuthClients(scope);
  if (list.isPending) return <Loading />;
  if (list.isError) return <ErrorState error={list.error} onRetry={() => list.refetch()} />;
  const mine = list.data.items.filter((c) => c.package_id === detail.package_id && c.scope === scope.kind);
  return (
    <div className="mt-2 space-y-3 border-t border-line pt-3">
      <p className="text-sm font-medium text-ink">{view === "workspace" ? "This workspace's sign-in apps" : `${orgName}'s sign-in apps`}</p>
      {mine.length > 0 ? (
        <ul className="space-y-3" aria-label="Sign-in apps of this app">
          {mine.map((c) => (
            <ClientRow
              key={c.id}
              scope={scope}
              c={c}
              canOwn={canRegister}
              def={detail.oauth_clients.find((d) => d.client_key === c.client_key)}
              appName={appDisplayName(detail)}
              appId={detail.app_id}
              linkApp={false}
            />
          ))}
        </ul>
      ) : (
        <p className="text-sm text-muted">None yet.</p>
      )}
      {canRegister && mine.length === 0 ? (
        <Button size="sm" variant="outline" onClick={onRegister}>
          Register a sign-in app for {view === "workspace" ? "this workspace" : orgName}
        </Button>
      ) : null}
    </div>
  );
}

/** The callback URL (for the OAuth app created at the provider) and, for apps that use it, the organization's external id. */
export function ValuesToCopy({ setup, orgName, showCallback }: { setup: AppSetup; orgName: string; showCallback: boolean }) {
  const appName = appDisplayName(setup);
  const { callback_url, external_id } = setup.values;
  if (!showCallback && !external_id) return null;
  return (
    <Card className="space-y-4 p-5" data-testid="setup-values">
      <h2 id="cp-h" className="font-display text-base font-semibold text-ink">
        Values to copy
      </h2>
      {showCallback ? <CopyValue label={`Callback URL, for the OAuth app you create in ${appName}`} value={callback_url} /> : null}
      {external_id ? (
        <CopyValue
          label={`External ID of ${orgName}, for the trust policy you create in ${appName}`}
          value={external_id}
          hint="It is not a secret: it keeps another organization from using your role."
        />
      ) : null}
    </Card>
  );
}

/** What the app can do for an agent: the former Details content, tools with their descriptions, hosts, instructions. */
export function WhatItCanDo({ detail }: { detail: AppDetail }) {
  const group = (label: string, g: string) => {
    const tools = detail.tools.filter((t) => t.group === g);
    return tools.length ? (
      <section aria-label={`${label} tools`}>
        <p className="text-label font-semibold uppercase tracking-label text-muted">{label}</p>
        <ul className="mt-1.5 list-disc space-y-1 pl-5 text-sm text-ink">
          {tools.map((t) => (
            <li key={t.id}>
              {t.name}
              {t.description ? <span className="text-ink-2"> — {t.description}</span> : null}
            </li>
          ))}
        </ul>
      </section>
    ) : null;
  };
  return (
    <Card className="space-y-4 p-5" data-testid="setup-can-do">
      <h2 id="can-h" className="font-display text-lg font-semibold text-ink">
        What {appDisplayName(detail)} can do for an agent
      </h2>
      <div className="grid gap-x-6 gap-y-4 sm:grid-cols-2">
        {group("Read", "read")}
        {group("Change", "write")}
      </div>
      {detail.hosts.length > 0 || detail.agent_instructions ? (
        <Altitude id="tech.details" level="setup">
        <details className="text-sm">
          <summary className="cursor-pointer font-medium text-ink">Technical details</summary>
          <div className="mt-3 space-y-3">
            {detail.hosts.length > 0 ? (
              <section aria-label="Hosts" className="space-y-1.5">
                <p className="font-medium text-ink">Hosts</p>
                <ul className="space-y-1.5">
                  {detail.hosts.map((h) => (
                    <li key={`${h.host}${h.path_prefix ?? ""}`} className="space-y-1">
                      <code className="break-all font-mono text-xs">
                        {h.host}
                        {h.path_prefix ?? ""}
                      </code>
                      {h.claim_status === "lost" ? (
                        <Notice tone="pending" role="status" className="text-xs">
                          Another app currently claims this host, so credentials are not injected for this app.
                        </Notice>
                      ) : null}
                    </li>
                  ))}
                </ul>
              </section>
            ) : null}
            {detail.agent_instructions ? (
              <section aria-label="Agent instructions" className="space-y-1.5">
                <p className="font-medium text-ink">Instructions shown to agents</p>
                <pre className="whitespace-pre-wrap break-words rounded-control border border-line bg-subtle p-3 text-xs text-ink-2">
                  {detail.agent_instructions}
                </pre>
              </section>
            ) : null}
          </div>
        </details>
        </Altitude>
      ) : null}
      {detail.api_docs ? (
        <section aria-label="API documentation">
          <Markdown source={detail.api_docs} />
        </section>
      ) : null}
    </Card>
  );
}

/** The setup guide with the viewer's values filled in (C-940), expanded, last. */
export function SetupGuide({ appId, scope }: { appId: string; scope: SetupScope }) {
  const guide = useAppGuide(appId, true, scope);
  if (guide.isPending) return <Loading />;
  if (guide.isError || !guide.data) return null;
  return (
    <Card className="space-y-3 p-5" data-testid="setup-guide">
      <h2 id="gd-h" className="font-display text-lg font-semibold text-ink">
        Setup guide
      </h2>
      <Markdown source={guide.data} />
    </Card>
  );
}
