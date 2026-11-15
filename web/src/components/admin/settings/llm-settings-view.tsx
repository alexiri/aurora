"use client";

import {
  ENABLED,
  ENDPOINTS,
  EndpointsControl,
  PREFERENCE,
  PreferenceControl,
  ProvidersControl,
} from "@/components/admin/settings/llm-controls";
import { SettingsSectionForm, type SettingsGroup } from "@/components/admin/settings/settings-section-form";
import { ErrorState, Loading } from "@/components/states";
import { Notice } from "@/components/ui/notice";
import { PageHeader } from "@/components/ui/page-header";
import { useSettingsSection } from "@/lib/api/settings";

/** The two groups of the page: which providers, then the endpoints (`15 §Instance settings`, row `llm`). */
export const LLM_GROUPS: SettingsGroup[] = [
  {
    title: "Providers",
    description:
      "Which providers this instance offers and the order a hosted agent picks one in. A provider's endpoints and keys appear in pickers only while it is enabled.",
    keys: [ENABLED, PREFERENCE],
  },
  {
    title: "Endpoints",
    description:
      "Where agents' LLM requests go. An endpoint defined in Helm is managed there; endpoints added here sit beside it. Saving applies at once, without a restart.",
    keys: [ENDPOINTS],
  },
];

/** Admin › Settings › LLM endpoints (SET-3, C-779): providers, preference order and endpoints with a connection test. */
export function LlmSettingsView() {
  const q = useSettingsSection("llm");
  if (q.isPending) return <Loading />;
  if (q.isError) return <ErrorState error={q.error} onRetry={() => q.refetch()} />;
  const section = q.data;
  const extra = (section.extra ?? {}) as { providers?: unknown };
  const available = Array.isArray(extra.providers) ? extra.providers.filter((p): p is string => typeof p === "string") : [];
  const byKey = (k: string) => section.settings.find((s) => s.key === k);
  const enabledEffective = byKey(ENABLED)?.value;
  const pref = byKey(PREFERENCE);
  return (
    <div className="max-w-page space-y-6">
      <PageHeader
        breadcrumb={[{ label: "Settings", href: "/admin/settings" }, { label: section.title }]}
        title={section.title}
        description={section.description}
      />
      <Notice tone="neutral" role="status" className="max-w-3xl">
        LLM keys are added by organizations and never stored in an endpoint. Removing an endpoint keeps their keys but
        stops them working; the save shows how many. See docs/operator/llm-endpoints.md.
      </Notice>
      <SettingsSectionForm
        key={section.generation}
        section={section}
        groups={LLM_GROUPS}
        onReload={() => void q.refetch()}
        controls={{
          [ENABLED]: (ctx) => (
            <ProvidersControl ctx={ctx} available={available} preference={{ value: pref?.value, locked: Boolean(pref?.locked) }} />
          ),
          [PREFERENCE]: (ctx) => <PreferenceControl ctx={ctx} enabledEffective={enabledEffective} />,
          [ENDPOINTS]: (ctx) => <EndpointsControl ctx={ctx} available={available} enabledEffective={enabledEffective} />,
        }}
      />
    </div>
  );
}
