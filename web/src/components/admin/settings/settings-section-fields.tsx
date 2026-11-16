"use client";

import { SettingsSectionForm, type SettingsGroup } from "@/components/admin/settings/settings-section-form";
import { ErrorState, Loading } from "@/components/states";
import { useSettingsSection } from "@/lib/api/settings";

/**
 * A settings section, or a subset of its keys, inside any page (C-971, `14` "Settings live in their topics").
 *
 * It carries the section's draft, preview, Helm locks, field errors and Save. Saving a subset sends the
 * section's whole document: the form draws every key of the section in its draft, shows only the chosen
 * ones, and the untouched keys go back with the value the server holds (see `SettingsSectionForm`).
 *
 * - `keys`: show only these keys, in one block titled `title` (default: the section's title).
 * - `groups`: show several titled blocks (each a subset of keys); wins over `keys`.
 */
export function SettingsSectionFields({
  section,
  keys,
  groups,
  title,
  description,
}: {
  section: string;
  keys?: string[];
  groups?: SettingsGroup[];
  title?: string;
  description?: string;
}) {
  const q = useSettingsSection(section);
  if (q.isPending) return <Loading />;
  if (q.isError) return <ErrorState error={q.error} onRetry={() => q.refetch()} />;
  const data = q.data;
  const shown: SettingsGroup[] | undefined =
    groups ?? (keys ? [{ title: title ?? data.title, description: description ?? data.description, keys }] : undefined);
  return (
    // Re-key on the generation so the form always starts from what the server holds.
    <SettingsSectionForm key={data.generation} section={data} groups={shown} onReload={() => void q.refetch()} />
  );
}
