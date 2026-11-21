"use client";

import { useQueryClient } from "@tanstack/react-query";
import { useParams } from "next/navigation";
import { useCallback, useState } from "react";

import { accountKeys } from "@/lib/api/accounts";
import { AccountsList } from "@/components/accounts/accounts-list";
import { ConnectInWorkspaceDialog } from "@/components/apps/org-apps-list";
import { OrgConnectJourney } from "@/components/apps/connect-journey/org-journey";
import { OrgGate } from "@/components/org/org-gate";
import { ConnectJourney } from "@/components/apps/connect-journey/connect-journey";
import { OAuthClientDialog } from "@/components/apps/oauth-clients-panel";
import { ErrorState, Loading } from "@/components/states";
import { AppIcon } from "@/components/ui/app-icon";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { PageHeader } from "@/components/ui/page-header";
import { WhoCan } from "@/components/ui/who-can";
import { appDisplayName } from "@/lib/api/app-name";
import { appKeys, useApp, useAppSetup, useConnectChannel, type SetupScope } from "@/lib/api/apps";
import { useOrg, useResolve, useWorkspace, type Org, type Workspace } from "@/lib/api/queries";
import { useWhoCan } from "@/lib/api/who-can";
import { setupActions, setupHeadline, type SetupView } from "@/lib/apps/setup-copy";
import { orgRoutes, wsRoutes } from "@/lib/routes";

import { SetupGuide, ValuesToCopy, WaysToConnect, WhatIsSetUp, WhatItCanDo } from "./sections";

/**
 * (P5-UX-9, C-932, `14 §Agents, accounts and access` §4) One page for one app, for a workspace (its owners) and for an
 * organization (its owners and admins): whether people can use the app, what is missing and who can fix it, and the
 * fixes the viewer's role allows, right here.
 */
function AppSetupView({
  view,
  appId,
  orgId,
  orgSlug,
  orgName,
  workspace,
  wsSlug,
}: {
  view: SetupView;
  appId: string;
  orgId: string;
  orgSlug: string;
  orgName: string;
  workspace?: Workspace;
  wsSlug?: string;
}) {
  const scope: SetupScope = workspace ? { workspaceId: workspace.id } : { orgId };
  const setup = useAppSetup(appId, scope);
  const detail = useApp(appId, workspace?.id);
  const qc = useQueryClient();
  const [registering, setRegistering] = useState(false);
  const [connecting, setConnecting] = useState(false);
  const [forOrg, setForOrg] = useState(false);
  const refresh = useCallback(() => {
    void qc.invalidateQueries({ queryKey: ["app-setup"] });
    void qc.invalidateQueries({ queryKey: accountKeys.all });
    void qc.invalidateQueries({ queryKey: appKeys.apps(workspace?.id) });
    void qc.invalidateQueries({ queryKey: ["connections"] });
  }, [qc, workspace?.id]);
  // Another dashboard tab finished a sign-in: this page's accounts and status follow.
  useConnectChannel(refresh);

  if (setup.isPending || detail.isPending) return <Loading />;
  if (setup.isError) return <ErrorState error={setup.error} onRetry={() => setup.refetch()} />;
  if (detail.isError) return <ErrorState error={detail.error} onRetry={() => detail.refetch()} />;
  const s = setup.data;
  const d = detail.data;
  const name = appDisplayName(s);
  const actions = setupActions(view, s);
  const credScope = workspace ? ({ kind: "workspace", id: workspace.id } as const) : ({ kind: "organization", id: orgId } as const);
  const usesClients = s.methods.some((m) => m.client_key);
  const usesCallback = s.methods.some((m) => m.kind === "oauth2");
  const appsHref = workspace ? wsRoutes(orgSlug, wsSlug!).apps("apps") : orgRoutes(orgSlug).apps("apps");

  return (
    <div className="max-w-page space-y-6" data-testid={view === "workspace" ? "ws-app-setup" : "org-app-setup"}>
      <PageHeader
        title={
          <span className="flex min-w-0 items-center gap-3">
            <AppIcon appId={s.app_id} name={name} hasDarkIcon={d.has_dark_icon} />
            <span className="min-w-0 break-words">{name}</span>
          </span>
        }
        description={
          <span className="flex flex-wrap items-center gap-x-2 gap-y-1">
            <span>{d.description}</span>
            <Badge tone={s.available ? "accent" : "neutral"}>
              {s.available ? `Available in ${orgName}` : `Not available in ${orgName}`}
            </Badge>
          </span>
        }
        breadcrumb={[{ label: "Apps & accounts", href: appsHref }, { label: name }]}
        actions={
          actions.connect ? (
            <Button onClick={() => setConnecting(true)} aria-label={`Connect ${name}`}>
              {view === "workspace" ? "Connect" : "Connect…"}
            </Button>
          ) : null
        }
      />

      <p className="max-w-3xl font-display text-xl font-medium leading-8 text-ink" data-testid="setup-headline">
        {setupHeadline(name, s.methods, view)}
      </p>

      <div className="flex flex-wrap items-start gap-5">
        <div className="flex min-w-0 flex-[3_1_34rem] flex-col gap-5">
          <WaysToConnect
            setup={s}
            view={view}
            orgName={orgName}
            orgSlug={orgSlug}
            packageId={d.package_id}
            actions={actions}
            onRegisterClient={() => setRegistering(true)}
          />
          <section aria-labelledby="acc-h" className="space-y-3">
            <h2 id="acc-h" className="font-display text-lg font-semibold text-ink">
              {name} accounts {view === "workspace" ? "here" : "for the whole organization"}
            </h2>
            <AccountsList
              scope={credScope}
              orgId={orgId}
              canOwn={view === "workspace" ? actions.connect : view === "organization"}
              appId={s.app_id}
            />
          </section>
          <WhatItCanDo detail={d} />
        </div>
        <aside className="flex min-w-0 flex-[2_1_20rem] flex-col gap-5">
          <WhatIsSetUp
            setup={s}
            detail={d}
            view={view}
            scope={credScope}
            orgName={orgName}
            orgSlug={orgSlug}
            actions={actions}
            usesClients={usesClients}
            onRegisterClient={() => setRegistering(true)}
          />
          <ValuesToCopy setup={s} orgName={orgName} showCallback={usesCallback} />
        </aside>
      </div>

      <SetupGuide appId={s.app_id} scope={scope} />

      {registering ? (
        <OAuthClientDialog
          scope={credScope}
          appId={s.app_id}
          orgId={orgId}
          onClose={() => {
            setRegistering(false);
            refresh();
          }}
        />
      ) : null}
      {connecting && workspace ? (
        <ConnectJourney
          appId={s.app_id}
          workspaceId={workspace.id}
          orgId={orgId}
          onClose={() => {
            setConnecting(false);
            refresh();
          }}
        />
      ) : null}
      {connecting && !workspace ? (
        <ConnectInWorkspaceDialog
          orgId={orgId}
          orgSlug={orgSlug}
          app={s}
          canOrg={view === "organization"}
          onOrg={() => {
            setConnecting(false);
            setForOrg(true);
          }}
          onClose={() => setConnecting(false)}
        />
      ) : null}
      {forOrg ? (
        <OrgConnectJourney
          orgId={orgId}
          appId={s.app_id}
          onClose={() => {
            setForOrg(false);
            refresh();
          }}
        />
      ) : null}
    </div>
  );
}

/** The workspace's page, `/o/<org>/w/<ws>/apps/<app>`, for workspace owners. */
export function WorkspaceAppSetupPage() {
  const { org, ws, app } = useParams<{ org: string; ws: string; app: string }>();
  const resolved = useResolve(org, ws);
  const wsId = resolved.data?.workspace?.id;
  const workspace = useWorkspace(wsId);
  const orgData = useOrg(resolved.data?.org.id);
  if (resolved.isPending || (wsId && workspace.isPending) || orgData.isPending) return <Loading />;
  if (resolved.isError) return <ErrorState error={resolved.error} onRetry={() => resolved.refetch()} />;
  if (!wsId) return <ErrorState error={{ code: "not_found" }} />;
  if (workspace.isError) return <ErrorState error={workspace.error} onRetry={() => workspace.refetch()} />;
  if (orgData.isError) return <ErrorState error={orgData.error} onRetry={() => orgData.refetch()} />;
  const w = workspace.data!;
  if (w.my_role !== "owner") return <OwnersOnly workspaceId={w.id} />;
  return (
    <AppSetupView view="workspace" appId={app} orgId={w.org_id} orgSlug={org} orgName={orgData.data!.name} workspace={w} wsSlug={ws} />
  );
}

/** A member opened the page: it says who can (C-981), never a bare refusal. */
function OwnersOnly({ workspaceId }: { workspaceId: string }) {
  const who = useWhoCan({ workspaceId }, "workspace_owner", "set_up");
  return (
    <div className="max-w-page space-y-4">
      <PageHeader title="Set up an app" />
      <WhoCan value={who} lead />
    </div>
  );
}

/** The organization's page, `/o/<org>/apps/<app>`, for organization owners and admins. */
export function OrgAppSetupPage() {
  const { org: slug, app } = useParams<{ org: string; app: string }>();
  return (
    <OrgGate slug={slug} allow={["owner", "admin"]}>
      {(o: Org) => <AppSetupView view="organization" appId={app} orgId={o.id} orgSlug={slug} orgName={o.name} />}
    </OrgGate>
  );
}
