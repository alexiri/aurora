"use client";

import Link from "next/link";

import { ErrorState, Loading } from "@/components/states";
import { PageHeader } from "@/components/ui/page-header";
import { ItemList, ListRow } from "@/components/ui/section";
import { useSettingsIndex, useSettingsSections } from "@/lib/api/settings";
import { adminRoutes } from "@/lib/routes";
import { valueWords } from "@/lib/settings-words";

/**
 * (P5-REFINE-18 #1, rec 9) Every value the Helm values set, in one place: the setting in words, its current value in
 * words, and the Helm key to change. Every "Set by the installation" pill links to its row here (`#<key>`).
 */
export function SetInHelmView() {
  const index = useSettingsIndex();
  const sections = [...new Set((index.data?.locked ?? []).map((l) => l.section))];
  const loaded = useSettingsSections(sections);
  if (index.isPending) return <Loading />;
  if (index.isError) return <ErrorState error={index.error} onRetry={() => index.refetch()} />;
  const locked = index.data.locked;
  const byKey = new Map<string, { label: string; value: string }>();
  for (const q of loaded) {
    const s = q.data;
    if (!s) continue;
    for (const st of s.settings) {
      const field = s.fields.find((f) => f.key === st.key);
      byKey.set(st.key, { label: field?.label ?? st.key, value: valueWords(field, st.value) });
    }
  }
  return (
    <div className="max-w-page space-y-6">
      <PageHeader
        title="Set in Helm"
        description="These values are set when Aurora is installed. To change one, change the Helm values and upgrade."
      />
      {locked.length === 0 ? (
        <p className="text-sm text-ink-2" role="status">
          Nothing is set in Helm. Every setting can be changed in Administration.
        </p>
      ) : (
        <ItemList aria-label="Values set in Helm">
          {locked.map((l) => {
            const id = `${l.key}${l.entry_id ? `.${l.entry_id}` : ""}`;
            const known = byKey.get(l.key);
            return (
              <ListRow key={id}>
                <div id={l.key} className="min-w-0 scroll-mt-24 space-y-1">
                  <p className="font-medium text-ink">
                    {known?.label ?? "A setting"}
                    {l.entry_id ? <span className="text-ink-2"> · {l.entry_id}</span> : null}
                  </p>
                  {known && !l.entry_id ? <p className="text-sm text-ink">{known.value}</p> : null}
                  <p className="text-xs text-muted">
                    Set in your Helm values: <code className="font-mono">{l.helm_key}</code>
                  </p>
                  <p className="text-xs">
                    <Link href={adminRoutes.sectionPage(l.section, l.key)} className="text-ink-2 underline-offset-2 hover:underline">
                      Where it is used
                    </Link>
                  </p>
                </div>
              </ListRow>
            );
          })}
        </ItemList>
      )}
    </div>
  );
}
