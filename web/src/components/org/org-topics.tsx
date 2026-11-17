"use client";

import { useParams } from "next/navigation";
import type { ReactNode } from "react";

import { AvailabilityView } from "@/components/apps/availability-view";
import { InstancesView } from "@/components/apps/instances-view";
import { OAuthClientsPanel } from "@/components/apps/oauth-clients-panel";
import { ChannelsView } from "@/components/channels/channels-view";
import { AccountsPanel } from "@/components/accounts/accounts-panel";
import { GovernanceView, type Tab as GovernancePanel } from "@/components/governance/governance-view";
import { AppsPosturePanel } from "@/components/governance/apps-posture-panel";
import { GroupsView } from "@/components/groups/groups-view";
import { OrgAuditView } from "@/components/org/audit-view";
import { OrgDirectoryView } from "@/components/org/directory-view";
import { OrgKeysView } from "@/components/org/keys-view";
import { OrgGate } from "@/components/org/org-gate";
import { MembersView } from "@/components/org/members-view";
import { OrgDangerSections, GeneralForm, PreviewsForm } from "@/components/org/settings-view";
import { OrgRetentionForm } from "@/components/org/retention-form";
import { OrgStorageView } from "@/components/org/storage-view";
import { OrgInternet } from "@/components/internet/org-internet";
import { OrgSkills } from "@/components/skills/org-skills";
import { TopicPage, type TopicTab } from "@/components/topic-page";
import { EmbeddedPage, PageHeader } from "@/components/ui/page-header";
import { Section } from "@/components/ui/section";
import { useScimSettings } from "@/lib/api/scim";
import type { Org } from "@/lib/api/queries";
import { type TABS } from "@/lib/routes";

type Tabs<K extends keyof typeof TABS> = (typeof TABS)[K][number];

const ADMINS = ["owner", "admin"] as const;
const ALL_ROLES = ["owner", "admin", "member"] as const;
const isAdmin = (org: Org) => org.my_role === "owner" || org.my_role === "admin";

function useOrgSlug(): string {
  return useParams<{ org: string }>().org;
}

/** A topic page for owners and admins: members are refused, as for any page their role cannot open. */
function AdminTopic({ children }: { children: (org: Org, slug: string) => ReactNode }) {
  const slug = useOrgSlug();
  return (
    <OrgGate slug={slug} allow={ADMINS}>
      {(org) => children(org, slug)}
    </OrgGate>
  );
}

/** One governance panel group (AI models, Internet access, BYO) of an organization, read-only for members. */
function Governance({ org, slug, panels }: { org: Org; slug: string; panels: GovernancePanel[] }) {
  return <GovernanceView orgId={org.id} orgSlug={slug} canEdit={isAdmin(org)} panels={panels} />;
}

export function OrgAgentsPage() {
  return (
    <AdminTopic>
      {(org, slug) => (
        <TopicPage<Tabs<"orgAgents">>
          idBase="org-agents"
          title="Agents"
          description="Rules for bring-your-own agents, the skills every agent has, and previews."
          tabs={[
            { id: "byo", label: "BYO", content: <Governance org={org} slug={slug} panels={["byo"]} /> },
            { id: "skills", label: "Skills", content: <OrgSkills /> },
            { id: "previews", label: "Previews", content: <PreviewsForm org={org} /> },
          ]}
        />
      )}
    </AdminTopic>
  );
}

export function OrgAppsPage() {
  return (
    <AdminTopic>
      {(org, slug) => (
        <TopicPage<Tabs<"orgApps">>
          idBase="org-apps"
          title="Apps & accounts"
          description="The accounts shared with the whole organization, and the apps people can connect."
          tabs={[
            {
              id: "accounts",
              label: "Accounts",
              content: <AccountsPanel scope={{ kind: "organization", id: org.id }} orgId={org.id} canOwn />,
            },
            {
              id: "apps",
              label: "Apps",
              content: (
                <EmbeddedPage headed>
                  <div className="space-y-8">
                    <AvailabilityView orgId={org.id} orgSlug={slug} canEdit />
                    <AppsPosturePanel orgId={org.id} orgSlug={slug} readOnly={false} />
                  </div>
                </EmbeddedPage>
              ),
            },
            {
              id: "oauth-clients",
              label: "OAuth clients",
              content: <OAuthClientsPanel scope={{ kind: "organization", id: org.id }} canOwn orgId={org.id} />,
            },
            { id: "self-hosted", label: "Self-hosted apps", content: <InstancesView scope={{ kind: "organization", id: org.id }} canManage /> },
          ]}
        />
      )}
    </AdminTopic>
  );
}

/** AI models: the model rules, with the notice for AI keys that no longer comply. Members read it. */
export function OrgAiModelsPage() {
  const slug = useOrgSlug();
  return (
    <OrgGate slug={slug} allow={ALL_ROLES}>
      {(org) => (
        <div className="max-w-page-narrow space-y-4">
          <PageHeader
            title="AI models"
            description={
              isAdmin(org)
                ? "Which AI models the agents of this organization may use."
                : "These rules apply to your organization. Only owners and admins can change them."
            }
            className="mb-2"
          />
          <Governance org={org} slug={slug} panels={["models", "compliance"]} />
        </div>
      )}
    </OrgGate>
  );
}

/** Internet access (C-965): the summary, Common changes, rules, the limits above and below, one Apply. */
export function OrgInternetAccessPage() {
  const slug = useOrgSlug();
  return (
    <OrgGate slug={slug} allow={ALL_ROLES}>
      {(org) => <OrgInternet org={org} slug={slug} />}
    </OrgGate>
  );
}

export function OrgPeoplePage() {
  return (
    <AdminTopic>
      {(org, slug) => (
        <DirectoryAwareTopic org={org} slug={slug} />
      )}
    </AdminTopic>
  );
}

/** People: Directory sync is for owners, and only where the instance has `scim.enabled` (docs/14). */
function DirectoryAwareTopic({ org, slug }: { org: Org; slug: string }) {
  const isOwner = org.my_role === "owner";
  const scim = useScimSettings(isOwner ? org.id : undefined);
  const directory = isOwner && scim.data?.available === true;
  const tabs: TopicTab<Tabs<"orgPeople">>[] = [
    { id: "members", label: "Members", content: <MembersView orgId={org.id} viewer={org.my_role} /> },
    { id: "groups", label: "Groups", content: <GroupsView orgId={org.id} orgSlug={slug} /> },
    ...(directory ? [{ id: "directory-sync" as const, label: "Directory sync", content: <OrgDirectoryView org={org} isOwner /> }] : []),
  ];
  return <TopicPage idBase="org-people" title="People" description="Who can use this organization, and the groups they belong to." tabs={tabs} />;
}

export function OrgChatPlatformsPage() {
  return (
    <AdminTopic>
      {(org) => (
        <ChannelsView orgId={org.id} />
      )}
    </AdminTopic>
  );
}

export function OrgDataPage() {
  return (
    <AdminTopic>
      {(org) => (
        <TopicPage<Tabs<"orgData">>
          idBase="org-data"
          title="Data"
          description="How long conversations and files are kept, how much the organization stores, and its audit log."
          tabs={[
            {
              id: "retention",
              label: "Retention",
              content: (
                <OrgRetentionForm orgId={org.id} />
              ),
            },
            { id: "storage", label: "Storage", content: <OrgStorageView orgId={org.id} /> },
            { id: "audit", label: "Audit log", content: <OrgAuditView orgId={org.id} /> },
          ]}
        />
      )}
    </AdminTopic>
  );
}

export function OrgSettingsPage() {
  return (
    <AdminTopic>
      {(org) => (
        <TopicPage<Tabs<"orgSettings">>
          idBase="org-settings"
          title="Settings"
          description="The organization's name and limits, its API keys, and deletion."
          width="max-w-page-narrow"
          tabs={[
            {
              id: "general",
              label: "General",
              content: (
                <Section id="org-general" title="General" card>
                  <GeneralForm key={`${org.slug}:${org.name}`} org={org} isOwner={org.my_role === "owner"} />
                </Section>
              ),
            },
            { id: "api-keys", label: "API keys", content: <OrgKeysView orgId={org.id} /> },
            { id: "danger", label: "Danger zone", content: <OrgDangerSections org={org} /> },
          ]}
        />
      )}
    </AdminTopic>
  );
}
