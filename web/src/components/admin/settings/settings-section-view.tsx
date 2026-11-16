"use client";

import type { ReactNode } from "react";

import {
  AgentsSettingsView,
  ApprovalsSettingsView,
  LimitsSettingsView,
  PreviewsSettingsView,
  RetentionSettingsView,
  ScimSettingsView,
  SkillsSettingsView,
  TerminalSettingsView,
} from "@/components/admin/settings/operational-settings-views";
import { SecretStoresView } from "@/components/admin/secret-stores-view";
import { EmailSettingsView } from "@/components/admin/settings/email-settings-view";
import { SignInSettingsView } from "@/components/admin/settings/sign-in-settings-view";
import { GovernanceSettingsView } from "@/components/admin/settings/governance-settings-view";
import { LlmSettingsView } from "@/components/admin/settings/llm-settings-view";
import { SettingsSectionForm } from "@/components/admin/settings/settings-section-form";
import { ErrorState, Loading } from "@/components/states";
import { PageHeader } from "@/components/ui/page-header";
import { sectionFromSlug, useSettingsSection } from "@/lib/api/settings";

const OPERATIONAL_VIEWS: Record<string, ReactNode> = {
  agents: <AgentsSettingsView />,
  approvals: <ApprovalsSettingsView />,
  retention: <RetentionSettingsView />,
  skills: <SkillsSettingsView />,
  scim: <ScimSettingsView />,
  previews: <PreviewsSettingsView />,
  terminal: <TerminalSettingsView />,
  limits: <LimitsSettingsView />,
};

function when(iso: string | null | undefined): string {
  return iso ? new Date(iso).toLocaleString() : "never";
}

/** Admin › Settings › <section> (SET-1): the generic page; sections with their own editors replace the form. */
export function SettingsSectionView({ slug }: { slug: string }) {
  // FIX-124: the secret stores have their own editor (health, Test connection, migration).
  if (sectionFromSlug(slug) === "secrets") return <SecretStoresView />;
  // SET-2: the instance governance ceiling, with where organizations stand beneath the form.
  if (sectionFromSlug(slug) === "governance") return <GovernanceSettingsView />;
  // SET-3: providers, preference order and endpoints, with a connection test.
  if (sectionFromSlug(slug) === "llm") return <LlmSettingsView />;
  // SET-4: sign-up, sessions and identity providers; the mailer with a test message.
  if (sectionFromSlug(slug) === "sign_in") return <SignInSettingsView />;
  if (sectionFromSlug(slug) === "email") return <EmailSettingsView />;
  // SET-5: the operational sections, each a self-contained page (no sidebar link: reached from the index).
  const operational = OPERATIONAL_VIEWS[sectionFromSlug(slug)];
  if (operational) return operational;
  return <GenericSectionView slug={slug} />;
}

function GenericSectionView({ slug }: { slug: string }) {
  const q = useSettingsSection(sectionFromSlug(slug));
  if (q.isPending) return <Loading />;
  if (q.isError) return <ErrorState error={q.error} onRetry={() => q.refetch()} />;
  const section = q.data;
  return (
    <div className="max-w-page space-y-6">
      <PageHeader
        breadcrumb={[{ label: "Settings", href: "/admin/settings" }, { label: section.title }]}
        title={section.title}
        description={`${section.description} Last change: ${section.updated_by_label ? `${section.updated_by_label}, ` : ""}${when(section.updated_at)}.`}
      />
      {/* Re-key on the generation so the form always starts from what the server holds. */}
      <SettingsSectionForm key={section.generation} section={section} onReload={() => void q.refetch()} />
    </div>
  );
}
