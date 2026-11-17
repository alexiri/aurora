"use client";

import { useMemo, useState } from "react";

import { AppGrantEditor } from "@/components/agents/app-grant-editor";
import { AppIcon } from "@/components/ui/app-icon";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { appDisplayName } from "@/lib/api/app-name";
import { useApps, type AppSummary } from "@/lib/api/apps";
import type { Account } from "@/lib/api/accounts";

/**
 * (FIX-154, C-896, C-897) Apps that need no sign-in, on an agent's Access tab: each available one the workspace has no
 * account of is offered here, and **Give <Agent> access** creates (or reuses) its account and gives it in one save. Once
 * given, it is an ordinary account row.
 */
export function FreeApps({
  agent,
  workspaceId,
  accounts,
}: {
  agent: { id: string; name: string };
  workspaceId: string;
  accounts: Account[];
}) {
  const apps = useApps(workspaceId);
  const [giving, setGiving] = useState<AppSummary | null>(null);
  const free = useMemo(() => {
    const have = new Set(accounts.map((a) => a.app?.app_id).filter(Boolean));
    return (apps.data?.items ?? []).filter((a) => a.credential_free && a.available && !have.has(a.app_id));
  }, [apps.data, accounts]);
  if (free.length === 0) return null;
  return (
    <>
      <ul className="space-y-3" aria-label="Apps that need no sign-in">
        {free.map((a) => (
          <li key={a.app_id} className="flex flex-wrap items-center gap-3 rounded-card border border-line bg-surface p-4">
            <AppIcon appId={a.app_id} name={appDisplayName(a)} hasDarkIcon={a.has_dark_icon} size={32} />
            <div className="min-w-0 flex-1 space-y-0.5">
              <div className="flex flex-wrap items-center gap-2">
                <span className="break-words font-display text-base font-semibold text-ink">{appDisplayName(a)}</span>
                <Badge tone="accent">No sign-in needed</Badge>
              </div>
              <p className="text-sm text-ink-2">{a.description}</p>
            </div>
            <Button variant="outline" size="sm" aria-label={`Give ${agent.name} access to ${appDisplayName(a)}`} onClick={() => setGiving(a)}>
              Give {agent.name} access
            </Button>
          </li>
        ))}
      </ul>
      {giving ? <AppGrantEditor key={giving.app_id} agentId={agent.id} wsId={workspaceId} app={giving} onClose={() => setGiving(null)} /> : null}
    </>
  );
}
