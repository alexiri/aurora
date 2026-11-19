"use client";

import { useParams, useRouter, useSearchParams } from "next/navigation";
import type { ReactNode } from "react";

import { AppsTab } from "@/components/apps/apps-tab";
import { AccountsPanel } from "@/components/accounts/accounts-panel";
import { WorkspaceFreeze } from "@/components/freeze/freeze-sections";
import { WorkspaceInternet } from "@/components/internet/workspace-internet";
import { AccessTab } from "@/components/settings/access-tab";
import { AddPeople } from "@/components/settings/add-people";
import { useAccess } from "@/lib/api/account";
import { AuditTab } from "@/components/settings/audit-tab";
import { DangerTab } from "@/components/settings/danger-tab";
import { GeneralTab } from "@/components/settings/general-tab";
import { KeysTab } from "@/components/settings/keys-tab";
import { ErrorState, Loading } from "@/components/states";
import { TopicPage, type TopicTab } from "@/components/topic-page";
import { PageHeader } from "@/components/ui/page-header";
import { useResolve, useWorkspace, useWorkspaces, type Workspace } from "@/lib/api/queries";
import { orgRoutes, wsRoutes, type TABS } from "@/lib/routes";
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
            // (C-917) Members read the same list as text, with no controls.
            content: <AccountsPanel scope={{ kind: "workspace", id: w.id }} orgId={w.org_id} canOwn={canOwn} />,
          },
          {
            id: "apps",
            label: "Apps",
            // (P5-UX-9, C-932) A workspace's OAuth clients are managed on each app's setup page, not here.
            content: <AppsTab workspaceId={w.id} orgId={w.org_id} canOwn={canOwn} />,
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

/** Internet access (C-965): the summary, Common changes, rules, the limits from above with Tighten…, one Apply. */
export function WorkspaceInternetAccessPage() {
  return <WorkspaceGate>{(w, org, ws) => <WorkspaceInternet workspace={w} orgSlug={org} wsSlug={ws} />}</WorkspaceGate>;
}

/**
 * The People page's body. `GET /v1/workspaces/{ws}` does not say whose a personal workspace is; the organization's
 * workspace list does (`personal_of`), so a personal workspace has no Groups section (P5-POLISH-11).
 */
function PeopleBody({ w, org, ws, addOpen }: { w: Workspace; org: string; ws: string; addOpen: boolean }) {
  const list = useWorkspaces(w.org_id);
  const personal = !!w.personal_of || !!list.data?.items.find((x) => x.id === w.id)?.personal_of;
  // (P5-POLISH-11, D6) **Add people** is on the header's line; the list's query is shared, so this costs no request.
  const access = useAccess(w.id);
  return (
    <div className="max-w-page space-y-4">
      <PageHeader
        title="People"
        description="Who can use this workspace."
        className="mb-2"
        actions={
          <AddPeople
            wsId={w.id}
            orgId={w.org_id}
            workspacePath={wsRoutes(org, ws).base}
            hasApprover={!!access.data?.items.some((b) => b.role === "approver")}
            initiallyOpen={addOpen}
          />
        }
      />
      <AccessTab wsId={w.id} orgId={w.org_id} canOwn personal={personal} people={{ workspacePath: wsRoutes(org, ws).base, initiallyOpen: addOpen }} />
    </div>
  );
}

/** People: who can use this workspace (workspace owners). */
export function WorkspacePeoplePage() {
  // `?add=1` (Welcome's **Add people**) opens the dialog.
  const addOpen = useSearchParams().get("add") === "1";
  return (
    <WorkspaceGate>
      {(w, org, ws) =>
        w.my_role === "owner" ? (
          <PeopleBody w={w} org={org} ws={ws} addOpen={addOpen} />
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
