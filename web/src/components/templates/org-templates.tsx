"use client";

import { useState } from "react";

import { EmptyState, ErrorState, Loading } from "@/components/states";
import { ErrorNotice } from "@/components/ui/notice";
import { Switch } from "@/components/ui/switch";
import { ApiError } from "@/lib/api/client";
import { useOrgTemplates, useSetOrgTemplate } from "@/lib/api/agent-templates";

/** The one sentence on top of the organization's Templates tab (C-1115). */
export const ORG_TEMPLATES_SENTENCE =
  "Workspace members can create agents from these templates. Turning one off hides it; agents already made from it keep working.";

/**
 * Organization › Agents › Templates (C-1115): every offered template with its summary and a switch
 * "Offered in <org>". Under the instance's kill switch (`agents.templates.enabled`, the org routes answer
 * 404 `templates_disabled`) the tab says so in words instead of a red error.
 */
export function OrgTemplates({ orgId, orgName }: { orgId: string; orgName: string }) {
  const q = useOrgTemplates(orgId);
  const set = useSetOrgTemplate(orgId);
  const [error, setError] = useState<unknown>(null);
  const items = q.data?.items ?? [];
  // (C-1109) The kill switch: the list route refuses with 404 `templates_disabled`, in words.
  const disabled = q.error instanceof ApiError && q.error.code === "templates_disabled";
  return (
    <div className="space-y-4" data-testid="org-templates">
      <p className="text-sm text-ink-2">{ORG_TEMPLATES_SENTENCE}</p>
      {q.isPending ? <Loading /> : null}
      {disabled ? (
        <EmptyState title="Agent templates are turned off">
          <p className="text-sm text-ink-2">Agent templates are turned off on this instance. An Aurora administrator can turn them on again.</p>
        </EmptyState>
      ) : null}
      {q.isError && !disabled ? <ErrorState error={q.error} onRetry={() => q.refetch()} /> : null}
      <ErrorNotice error={error} />
      {q.data && items.length === 0 ? (
        <EmptyState title="Nothing offered yet">
          <p className="text-sm text-ink-2">Templates appear here when an instance administrator offers one.</p>
        </EmptyState>
      ) : null}
      {items.length > 0 ? (
        <ul aria-label={`Templates offered in ${orgName}`} className="space-y-2">
          {items.map((t) => (
            <li key={t.template_id} className="flex items-start justify-between gap-4 rounded-card border border-line bg-surface p-4" data-testid="org-template">
              <div className="min-w-0 space-y-0.5">
                <p className="font-medium text-ink [overflow-wrap:anywhere]">{t.name}</p>
                <p className="min-w-0 text-sm text-ink-2 [overflow-wrap:anywhere]">{t.summary}</p>
              </div>
              <Switch
                checked={t.enabled}
                disabled={set.isPending}
                aria-label={`Offered in ${orgName}: ${t.name}`}
                onCheckedChange={(checked) => {
                  setError(null);
                  set.mutate({ templateId: t.template_id, enabled: checked }, {
                    onError: (e) => {
                      setError(e);
                      void q.refetch();
                    },
                  });
                }}
              />
            </li>
          ))}
        </ul>
      ) : null}
    </div>
  );
}
