"use client";

import { useParams, useRouter } from "next/navigation";
import type { ReactNode } from "react";

import { AppsTab } from "@/components/apps/apps-tab";
import { OAuthClientsPanel } from "@/components/apps/oauth-clients-panel";
import { AccountsPanel } from "@/components/credentials/credentials-view";
import { WorkspaceFreeze } from "@/components/freeze/freeze-sections";
import { WorkspacePolicy } from "@/components/policy/workspace-policy";
import { AccessTab } from "@/components/settings/access-tab";
import { AuditTab } from "@/components/settings/audit-tab";
import { DangerTab } from "@/components/settings/danger-tab";
import { GeneralTab } from "@/components/settings/general-tab";
import { KeysTab } from "@/components/settings/keys-tab";
import { ErrorState, Loading } from "@/components/states";
import { TopicPage, type TopicTab } from "@/components/topic-page";
import { Notice } from "@/components/ui/notice";
import { WhoCan } from "@/components/ui/who-can";
import { PageHeader } from "@/components/ui/page-header";
import { Section } from "@/components/ui/section";
import { useResolve, useWorkspace, type Workspace } from "@/lib/api/queries";
import { orgRoutes, type TABS } from "@/lib/routes";
import { forbiddenError, whoCanFromContacts } from "@/lib/who-can";

type Tabs<K extends keyof typeof TABS> = (typeof TABS)[K][number];

/** Resolves the workspace in the URL and loads it; children get it with the viewer's role. */
function WorkspaceGate({ children }: { children: (w: Workspace, org: string, ws: string) => ReactNode }) {
  const { org, ws } = useParams<{ org: string; ws: string }>();
  const resolved = useResolve(org, ws);
  const wsId = resolved.data?.workspace?.id;
  const workspace = useWorkspace(wsId);
  if (resolved.isPending || (wsId && workspace.isPending)) return <Loading />;
  if (resolved.isError) return <ErrorState error={resolved.error} onRetry={() => resolved.refetch()} />;
  if (!wsId) return <ErrorState error={{ code: "not_found" }} />;
  if (workspace.isError) return <ErrorState error={workspace.error} onRetry={() => workspace.refetch()} />;
  return <>{children(workspace.data!, org, ws)}</>;
}

/** Apps & accounts: the workspace's accounts and the app catalogue. Members read the catalogue only until P5-UX-4. */
export function WorkspaceAppsPage() {
  return (
    <WorkspaceGate>
      {(w) => {
        const canOwn = w.my_role === "owner";
        const tabs: TopicTab<Tabs<"wsApps">>[] = [
          {
            id: "accounts",
            label: "Accounts",
            content: canOwn ? (
              <AccountsPanel scope={{ kind: "workspace", id: w.id }} orgId={w.org_id} canOwn />
            ) : (
              <Notice role="status">
                <WhoCan lead value={whoCanFromContacts("workspace_owner", w.contacts, "add_account")} className="text-inherit" />
              </Notice>
            ),
          },
          {
            id: "apps",
            label: "Apps",
            content: (
              <div className="space-y-8">
                <AppsTab workspaceId={w.id} orgId={w.org_id} canOwn={canOwn} />
                {/* Until the per-app setup pages hold them (P5-UX-9), a workspace's OAuth clients are reached from here. */}
                {canOwn ? (
                  <Section id="ws-oauth-clients" title="OAuth clients" description="App registrations of this workspace.">
                    <OAuthClientsPanel scope={{ kind: "workspace", id: w.id }} canOwn orgId={w.org_id} />
                  </Section>
                ) : null}
              </div>
            ),
          },
        ];
        return (
          <TopicPage
            idBase="ws-apps"
            title="Apps & accounts"
            description="The accounts your agents can use, and the apps you can connect."
            tabs={tabs}
          />
        );
      }}
    </WorkspaceGate>
  );
}

/** Internet access: the workspace's rules (owners edit; the rules are not readable by members until P5-IA-2). */
export function WorkspaceInternetAccessPage() {
  return (
    <WorkspaceGate>
      {(w) =>
        w.my_role === "owner" ? (
          <WorkspacePolicy />
        ) : (
          <div className="max-w-page space-y-4">
            <PageHeader title="Internet access" description="Which websites agents can reach, and the rules that ask or block." className="mb-2" />
            <Notice role="status">Workspace owners set the rules for which websites agents can reach.</Notice>
          </div>
        )
      }
    </WorkspaceGate>
  );
}

/** People: who can use this workspace (workspace owners). */
export function WorkspacePeoplePage() {
  return (
    <WorkspaceGate>
      {(w) =>
        w.my_role === "owner" ? (
          <div className="max-w-page space-y-4">
            <PageHeader title="People" description="Who can use this workspace." className="mb-2" />
            <AccessTab wsId={w.id} orgId={w.org_id} canOwn />
          </div>
        ) : (
          <ErrorState error={forbiddenError(whoCanFromContacts("workspace_owner", w.contacts, "view_this"))} />
        )
      }
    </WorkspaceGate>
  );
}

/** Settings: name, API keys, audit log and deletion. */
export function WorkspaceSettingsPage() {
  const router = useRouter();
  return (
    <WorkspaceGate>
      {(w, org) => {
        const canOwn = w.my_role === "owner";
        if (!canOwn) return <ErrorState error={forbiddenError(whoCanFromContacts("workspace_owner", w.contacts, "view_this"))} />;
        return (
          <TopicPage<Tabs<"wsSettings">>
            idBase="ws-settings"
            title="Settings"
            description="The workspace's name, API keys and audit log."
            tabs={[
              { id: "general", label: "General", content: <GeneralTab key={w.updated_at} workspace={w} canOwn={canOwn} /> },
              { id: "api-keys", label: "API keys", content: <KeysTab wsId={w.id} /> },
              { id: "audit", label: "Audit", content: <AuditTab wsId={w.id} /> },
              {
                id: "danger",
                label: "Danger zone",
                content: (
                  <div className="space-y-6">
                    <WorkspaceFreeze wsId={w.id} name={w.name} orgId={w.org_id} frozen={w.frozen} />
                    <DangerTab wsId={w.id} slug={w.slug} onDeleted={() => router.replace(orgRoutes(org).home())} />
                  </div>
                ),
              },
            ]}
          />
        );
      }}
    </WorkspaceGate>
  );
}
