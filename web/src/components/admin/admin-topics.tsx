"use client";

import { AdminStorageView } from "@/components/account/admin-storage-view";
import { AdminUsersView } from "@/components/account/admin-users-view";
import { AdminInternet } from "@/components/internet/admin-internet";
import { AdminGuard } from "@/components/admin/admin-guard";
import { AddonSourcesView } from "@/components/admin/addon-sources-view";
import { AdminAuditView } from "@/components/admin/audit-view";
import { HostClaimsView } from "@/components/admin/host-claims-view";
import { AdminOrgsView } from "@/components/admin/orgs-view";
import { PlacementView } from "@/components/admin/placement-view";
import { PluginsView } from "@/components/admin/plugins-view";
import { AdminRunnersView } from "@/components/admin/runners-view";
import { GovernanceFields, GovernanceRulesFields } from "@/components/admin/settings/governance-settings-view";
import { AgentsDefaultsView, ComputersView, SkillsSettingsView } from "@/components/admin/settings/operational-settings-views";
import { ProviderRolesView } from "@/components/admin/settings/provider-roles-view";
import { SettingsIndexView } from "@/components/admin/settings/settings-index-view";
import { SettingsSectionView } from "@/components/admin/settings/settings-section-view";
import { SetupView } from "@/components/admin/setup-view";
import { TrustedKeysView } from "@/components/admin/trusted-keys-view";
import { InstancesView } from "@/components/apps/instances-view";
import { OAuthClientsPanel } from "@/components/apps/oauth-clients-panel";
import { ConnectorsAdminView } from "@/components/connectors/connectors-admin-view";
import { TopicPage } from "@/components/topic-page";
import { EmbeddedPage } from "@/components/ui/page-header";
import { Section } from "@/components/ui/section";
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
          { id: "defaults", label: "Defaults", content: <AgentsDefaultsView /> },
          { id: "byo", label: "BYO", content: <GovernanceFields part="byo" /> },
          { id: "computers", label: "Computers", content: <ComputersView /> },
          { id: "skills", label: "Skills", content: <SkillsSettingsView bare /> },
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
          { id: "endpoints", label: "Endpoints", content: <EmbeddedPage>{section("llm")}</EmbeddedPage> },
          { id: "rules", label: "Rules", content: <GovernanceRulesFields /> },
        ]}
      />
    </AdminGuard>
  );
}

export function AdminInternetAccessPage() {
  return (
    <AdminGuard>
      <AdminInternet />
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
              <div className="space-y-10">
                <Section id="sign-in-providers" title="How people sign in" description="Who can sign up, how long a sign-in lasts, and the providers people sign in with.">
                  {section("sign_in")}
                </Section>
                <Section
                  id="sign-in-roles"
                  title="Roles from the provider"
                  description="Which role values from the provider give which role in Aurora. Change a mapping on its provider."
                >
                  <ProviderRolesView />
                </Section>
                <Section
                  id="sign-in-directory"
                  title="Directory sync"
                  description="Lets your identity system add and remove people and groups automatically. Organization owners then turn it on for their organization."
                >
                  {section("scim")}
                </Section>
              </div>
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
      <ConnectorsAdminView />
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
          { id: "audit", label: "Audit log", content: <AdminAuditView /> },
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
