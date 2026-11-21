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
import { SetInHelmView } from "@/components/admin/set-in-helm-view";
import { AdminHealthPanel, SetupView } from "@/components/admin/setup-view";
import { TrustedKeysView } from "@/components/admin/trusted-keys-view";
import { InstancesView } from "@/components/apps/instances-view";
import { OAuthClientsPanel } from "@/components/apps/oauth-clients-panel";
import { ConnectorsAdminView } from "@/components/connectors/connectors-admin-view";
import { TopicPage } from "@/components/topic-page";
import { EmbeddedPage } from "@/components/ui/page-header";
import { Section } from "@/components/ui/section";
import { UsageView } from "@/components/usage/usage-view";
import { OutgoingWebhooksAdminSection } from "@/components/webhook/admin-webhook-section";
import type { TABS } from "@/lib/routes";

type Tabs<K extends keyof typeof TABS> = (typeof TABS)[K][number];

const section = (slug: string) => <SettingsSectionView slug={slug} />;

/** Administration › Overview: the setup checklist (health follows with the Overview rebuild). */
export function AdminOverviewPage() {
  return (
    <AdminGuard>
      <div>
        <SetupView />
      </div>
    </AdminGuard>
  );
}


export function AdminAppsPage() {
  return (
    <AdminGuard>
      <TopicPage<Tabs<"adminApps">>
        idBase="admin-apps"
        title="Apps"
        description="The app catalogue, the sign-in apps shared by every organization, and the apps people host themselves."
        tabs={[
          { id: "apps", label: "Apps", content: <PluginsView /> },
          { id: "oauth-clients", label: "Sign-in apps", content: <OAuthClientsPanel scope={{ kind: "instance" }} canOwn /> },
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
                  {/* (P5-WEBHOOK-4, C-913) The `outgoing_webhooks` settings section, through SET-5's generic section form. */}
                  <OutgoingWebhooksAdminSection />
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
      <TopicPage<"endpoints">
        idBase="admin-ai-models"
        title="AI endpoints"
        description="The AI providers and endpoints agents' models come from. The rules for AI keys are under What agents may do."
        tabs={[{ id: "endpoints", label: "Endpoints", content: <EmbeddedPage>{section("llm")}</EmbeddedPage> }]}
      />
    </AdminGuard>
  );
}

/**
 * (P5-REFINE-18 #1, rec 9) What agents may do: one page, four tabs. The preset card sits first on Internet access;
 * AI keys and rules and Bring your own are the governance parts that used to live under AI models and Agents.
 */
export function AdminInternetAccessPage() {
  return (
    <AdminGuard>
      <TopicPage<Tabs<"adminAgentPolicy">>
        idBase="admin-agent-policy"
        title="What agents may do"
        description="Which websites agents can reach, which AI keys they may use, whether people can bring their own agents, and the defaults every agent starts with."
        tabs={[
          { id: "internet-access", label: "Internet access", content: <AdminInternet /> },
          { id: "ai-keys", label: "AI keys and rules", content: <GovernanceRulesFields /> },
          { id: "byo", label: "Bring your own", content: <GovernanceFields part="byo" /> },
          {
            id: "defaults",
            label: "Defaults",
            content: (
              <div className="space-y-10">
                <AgentsDefaultsView />
                <SkillsSettingsView bare />
              </div>
            ),
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
        title="People & sign-in"
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
          { id: "usage", label: "Usage", content: <UsageView level={{ kind: "admin" }} /> },
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
        title="Health & runners"
        description="Whether Aurora is healthy, the computers agents run on, and this installation's own settings: secret stores, e-mail and rate limits."
        tabs={[
          { id: "health", label: "Health", content: <AdminHealthPanel /> },
          { id: "runners", label: "Runners", content: <AdminRunnersView /> },
          { id: "computers", label: "Computers", content: <ComputersView /> },
          { id: "secret-stores", label: "Secret stores", content: section("secrets") },
          { id: "email", label: "Email", content: section("email") },
          { id: "limits", label: "Rate limits", content: section("limits") },
        ]}
      />
    </AdminGuard>
  );
}

export function AdminSetInHelmPage() {
  return (
    <AdminGuard>
      <SetInHelmView />
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
