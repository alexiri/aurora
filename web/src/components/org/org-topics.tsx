"use client";

import { useParams } from "next/navigation";
import type { ReactNode } from "react";

import { OrgAppsTable } from "@/components/apps/org-apps-table";
import { InstancesView } from "@/components/apps/instances-view";
import { OAuthClientsPanel } from "@/components/apps/oauth-clients-panel";
import { ChannelsView } from "@/components/channels/channels-view";
import { AccountsPanel } from "@/components/accounts/accounts-panel";
import { OrgAiModels, OrgByo } from "@/components/governance/org-governance";
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
import { AskAdmin } from "@/components/ask-admin";
import { AltitudeScope, MoreControlsLine } from "@/components/ui/altitude";
import { useAltitude, useHiddenControls } from "@/lib/altitude";
import { ALTITUDE_REGISTRY, type AltitudeId } from "@/lib/altitude-registry";
import { useScimSettings } from "@/lib/api/scim";
import { useWorkspaces, type Org } from "@/lib/api/queries";
import { onlySentence, whoCanFromContacts } from "@/lib/who-can";
import { UsageView } from "@/components/usage/usage-view";
import { OutgoingWebhooksOrgSettings } from "@/components/webhook/org-webhook-settings";
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

export function OrgAgentsPage() {
  const slug = useOrgSlug();
  return (
    <OrgGate slug={slug} allow={ALL_ROLES}>
      {(org) =>
        // (P5-POLISH-9, D-390) A member reads the sentence and who changes it, not a red refusal.
        isAdmin(org) ? (
          <TopicPage<Tabs<"orgAgents">>
            idBase="org-agents"
            title="Agents"
            description="Rules for bring-your-own agents, the skills every agent has, and previews."
            tabs={[
              { id: "byo", label: "BYO", content: <OrgByo org={org} /> },
              { id: "skills", label: "Skills", content: <OrgSkills /> },
              { id: "previews", label: "Previews", content: <PreviewsForm org={org} /> },
            ]}
          />
        ) : (
          <div className="max-w-page-narrow space-y-4" data-testid="member-org-agents">
            <PageHeader title="Agents" description="These rules apply to your organization. Only owners and admins can change them." className="mb-2" />
            <OrgByo org={org} />
          </div>
        )
      }
    </OrgGate>
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
                  <div className="space-y-6">
                    {/* (P5-POLISH-9, E3) One table: status, Set up and a switch per app; the sign-in gaps and the radio are in it. */}
                    <OrgAppsTable orgId={org.id} orgSlug={slug} canEdit />
                    {/* (P5-WEBHOOK-4, C-913) The organization's switch and limits for Outgoing webhooks. */}
                    <OutgoingWebhooksOrgSettings org={org} />
                  </div>
                </EmbeddedPage>
              ),
            },
            {
              id: "oauth-clients",
              label: "Sign-in apps",
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
          <OrgAiModels org={org} />
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

/** The registry ids of the two tabs (`lib/altitude-registry.ts`); both are Expert, Set up where a provider is configured. */
const ORG_TAB_ALTITUDE = { groups: "org.groups", directory: "org.directory" } as const satisfies Record<string, AltitudeId>;

/** Tells the page's `AltitudeScope` how many tabs the altitude left out. */
function ReportHidden({ n }: { n: number }) {
  useHiddenControls(n);
  return null;
}

/** People: Directory sync is for owners, and only where the instance has `scim.enabled` (docs/14). */
function DirectoryAwareTopic({ org, slug }: { org: Org; slug: string }) {
  const isOwner = org.my_role === "owner";
  const scim = useScimSettings(isOwner ? org.id : undefined);
  const directory = isOwner && scim.data?.available === true;
  // (P5-REFINE-3, D-406) Groups and Directory sync are Expert unless a directory provider is configured (then Set up).
  // `org.groups` and `org.directory` are the registry ids; the tabs are data, so the hook decides, not `<Altitude>`.
  const alt = useAltitude();
  const level = directory ? "setup" : ALTITUDE_REGISTRY[ORG_TAB_ALTITUDE.groups].altitude;
  const showGroups = alt.allows(level);
  const showDirectory = directory && alt.allows(level);
  const hidden = (showGroups ? 0 : 1) + (directory && !showDirectory ? 1 : 0);
  const tabs: TopicTab<Tabs<"orgPeople">>[] = [
    { id: "members", label: "Members", content: <MembersView orgId={org.id} viewer={org.my_role} /> },
    ...(showGroups ? [{ id: "groups" as const, label: "Groups", content: <GroupsView orgId={org.id} orgSlug={slug} /> }] : []),
    ...(showDirectory ? [{ id: "directory-sync" as const, label: "Directory sync", content: <OrgDirectoryView org={org} isOwner /> }] : []),
  ];
  return (
    <AltitudeScope>
      <ReportHidden n={hidden} />
      <TopicPage idBase="org-people" title="People" description="Who can use this organization, and the groups they belong to." tabs={tabs} />
      <MoreControlsLine className="pt-4" />
    </AltitudeScope>
  );
}

export function OrgChatPlatformsPage() {
  const slug = useOrgSlug();
  return (
    <OrgGate slug={slug} allow={ALL_ROLES}>
      {/* (P5-APIGAP-2, D-399) A member reads who connects a chat platform and can ask them, not a refusal. */}
      {(org) => (isAdmin(org) ? <ChannelsView orgId={org.id} /> : <MemberChatPlatforms org={org} />)}
    </OrgGate>
  );
}

function MemberChatPlatforms({ org }: { org: Org }) {
  const workspaces = useWorkspaces(org.id);
  const who = whoCanFromContacts("org_admin", org.contacts, "connect_chat");
  return (
    <div className="max-w-page-narrow space-y-4" data-testid="member-chat-platforms">
      <PageHeader
        title="Chat platforms"
        description={`Owners and admins connect ${org.name} to a chat platform; once they have, agents can be added to its channels.`}
        className="mb-2"
      />
      <div className="flex flex-wrap items-center gap-3 rounded-control bg-subtle px-3 py-2">
        {/* (D-407, channels-3) The role says who can; the button asks them. Nobody is named. */}
        <p className="text-sm text-ink-2">{onlySentence(who)}</p>
        <AskAdmin workspaceId={workspaces.data?.items[0]?.id} subject="chat_platform" who={who} />
      </div>
    </div>
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
            { id: "usage", label: "Usage", content: <UsageView level={{ kind: "org", orgId: org.id, orgSlug: org.slug }} /> },
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
