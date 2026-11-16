"use client";

import { AdminStorageView } from "@/components/account/admin-storage-view";
import { AdminUsersView } from "@/components/account/admin-users-view";
import { AdminGuard } from "@/components/admin/admin-guard";
import { AddonSourcesView } from "@/components/admin/addon-sources-view";
import { AdminAuditView } from "@/components/admin/audit-view";
import { HostClaimsView } from "@/components/admin/host-claims-view";
import { AdminOrgsView } from "@/components/admin/orgs-view";
import { PlacementView } from "@/components/admin/placement-view";
import { PluginsView } from "@/components/admin/plugins-view";
import { AdminRunnersView } from "@/components/admin/runners-view";
import { GovernanceFields, GovernanceRulesFields } from "@/components/admin/settings/governance-settings-view";
import { SettingsIndexView } from "@/components/admin/settings/settings-index-view";
import { SettingsSectionView } from "@/components/admin/settings/settings-section-view";
import { SetupView } from "@/components/admin/setup-view";
import { TrustedKeysView } from "@/components/admin/trusted-keys-view";
import { InstancesView } from "@/components/apps/instances-view";
import { OAuthClientsPanel } from "@/components/apps/oauth-clients-panel";
import { ConnectorsAdminView } from "@/components/connectors/connectors-admin-view";
import { TopicPage } from "@/components/topic-page";
import { EmbeddedPage, PageHeader } from "@/components/ui/page-header";
import type { TABS } from "@/lib/routes";

type Tabs<K extends keyof typeof TABS> = (typeof TABS)[K][number];

const section = (slug: string) => <SettingsSectionView slug={slug} />;

/** Administration › Overview: the setup checklist (health follows with the Overview rebuild). */
export function AdminOverviewPage() {
  return (
    <AdminGuard>
      <div className="max-w-page">
        <SetupView />
      </div>
    </AdminGuard>
  );
}

export function AdminAgentsPage() {
  return (
    <AdminGuard>
      <TopicPage<Tabs<"adminAgents">>
        idBase="admin-agents"
        title="Agents"
        description="Defaults for every agent, bring-your-own agents, their computers and skills."
        tabs={[
          {
            id: "defaults",
            label: "Defaults",
            content: (
              <EmbeddedPage headed>
                <div className="space-y-8">
                  {section("agents")}
                  {section("approvals")}
                </div>
              </EmbeddedPage>
            ),
          },
          {
            id: "byo",
            label: "BYO",
            content: <GovernanceFields part="byo" />,
          },
          {
            id: "computers",
            label: "Computers",
            content: (
              <EmbeddedPage headed>
                <div className="space-y-8">
                  {section("terminal")}
                  {section("previews")}
                </div>
              </EmbeddedPage>
            ),
          },
          { id: "skills", label: "Skills", content: section("skills") },
        ]}
      />
    </AdminGuard>
  );
}

export function AdminAppsPage() {
  return (
    <AdminGuard>
      <TopicPage<Tabs<"adminApps">>
        idBase="admin-apps"
        title="Apps & accounts"
        description="The app catalogue, the OAuth clients shared by every organization, and the apps people host themselves."
        tabs={[
          { id: "apps", label: "Apps", content: <PluginsView /> },
          { id: "oauth-clients", label: "OAuth clients", content: <OAuthClientsPanel scope={{ kind: "instance" }} canOwn /> },
          {
            id: "self-hosted",
            label: "Self-hosted apps",
            content: <InstancesView scope={{ kind: "instance", all: true }} canManage />,
          },
          {
            id: "advanced",
            label: "Advanced",
            content: (
              <EmbeddedPage headed>
                <div className="space-y-10">
                  <section id="package-sources">
                    <AddonSourcesView />
                  </section>
                  <section id="publishers">
                    <TrustedKeysView />
                  </section>
                  <section id="host-conflicts">
                    <HostClaimsView />
                  </section>
                </div>
              </EmbeddedPage>
            ),
          },
        ]}
      />
    </AdminGuard>
  );
}

export function AdminAiModelsPage() {
  return (
    <AdminGuard>
      <TopicPage<Tabs<"adminAiModels">>
        idBase="admin-ai-models"
        title="AI models"
        description="The AI providers and endpoints, and the rules for which models agents may use."
        tabs={[
          { id: "endpoints", label: "Endpoints", content: section("llm") },
          { id: "rules", label: "Rules", content: <GovernanceRulesFields /> },
        ]}
      />
    </AdminGuard>
  );
}

export function AdminInternetAccessPage() {
  return (
    <AdminGuard>
      <TopicPage
        idBase="admin-internet"
        title="Internet access"
        description="Which websites agents can reach."
        tabs={[
          {
            id: "limits",
            label: "Limits",
            content: <GovernanceFields part="internet" />,
          },
        ]}
      />
    </AdminGuard>
  );
}

export function AdminPeoplePage() {
  return (
    <AdminGuard>
      <TopicPage<Tabs<"adminPeople">>
        idBase="admin-people"
        title="People"
        description="Who can use this instance, their organizations, and how they sign in."
        tabs={[
          { id: "users", label: "Users", content: <AdminUsersView /> },
          { id: "organizations", label: "Organizations", content: <AdminOrgsView /> },
          {
            id: "sign-in",
            label: "Sign-in",
            content: (
              <EmbeddedPage headed>
                <div className="space-y-8">
                  {section("sign_in")}
                  {section("scim")}
                </div>
              </EmbeddedPage>
            ),
          },
          { id: "new-users", label: "New users", content: <PlacementView /> },
        ]}
      />
    </AdminGuard>
  );
}

export function AdminChatPlatformsPage() {
  return (
    <AdminGuard>
      <div className="max-w-page space-y-4">
        <PageHeader title="Chat platforms" description="Mattermost and similar: where people can talk to agents." className="mb-2" />
        <ConnectorsAdminView />
      </div>
    </AdminGuard>
  );
}

export function AdminDataPage() {
  return (
    <AdminGuard>
      <TopicPage<Tabs<"adminData">>
        idBase="admin-data"
        title="Data"
        description="How long Aurora keeps things, what it stores, and the audit log."
        tabs={[
          { id: "retention", label: "Retention", content: section("retention") },
          { id: "storage", label: "Storage", content: <AdminStorageView /> },
          { id: "audit", label: "Audit", content: <AdminAuditView /> },
        ]}
      />
    </AdminGuard>
  );
}

export function AdminPlatformPage() {
  return (
    <AdminGuard>
      <TopicPage<Tabs<"adminPlatform">>
        idBase="admin-platform"
        title="Platform"
        description="The machinery of this instance: runners, secret stores, e-mail and rate limits."
        tabs={[
          { id: "runners", label: "Runners", content: <AdminRunnersView /> },
          { id: "secret-stores", label: "Secret stores", content: section("secrets") },
          { id: "email", label: "Email", content: section("email") },
          { id: "limits", label: "Rate limits", content: section("limits") },
        ]}
      />
    </AdminGuard>
  );
}

export function AdminAllSettingsPage() {
  return (
    <AdminGuard>
      <SettingsIndexView />
    </AdminGuard>
  );
}
