"use client";

import { SecretStoresView } from "@/components/admin/secret-stores-view";
import { SettingsSectionForm } from "@/components/admin/settings/settings-section-form";
import { ErrorState, Loading } from "@/components/states";
import { PageHeader } from "@/components/ui/page-header";
import { sectionFromSlug, useSettingsSection } from "@/lib/api/settings";

function when(iso: string | null | undefined): string {
  return iso ? new Date(iso).toLocaleString() : "never";
}

/** Admin › Settings › <section> (SET-1): the generic page; sections with their own editors replace the form. */
export function SettingsSectionView({ slug }: { slug: string }) {
  // FIX-124: the secret stores have their own editor (health, Test connection, migration).
  if (sectionFromSlug(slug) === "secrets") return <SecretStoresView />;
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
