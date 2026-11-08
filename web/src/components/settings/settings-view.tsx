"use client";

import { useRouter } from "next/navigation";
import { useState } from "react";

import { WorkspaceFreeze } from "@/components/freeze/freeze-sections";
import { ErrorState, Loading } from "@/components/states";
import { useResolve, useWorkspace } from "@/lib/api/queries";
import { cn } from "@/lib/utils";

import { AccessTab } from "./access-tab";
import { AuditTab } from "./audit-tab";
import { DangerTab } from "./danger-tab";
import { GeneralTab } from "./general-tab";
import { KeysTab } from "./keys-tab";

const TABS = [
  { id: "general", label: "General", ownerOnly: false },
  { id: "access", label: "Access", ownerOnly: true },
  { id: "keys", label: "API keys", ownerOnly: true },
  { id: "audit", label: "Audit", ownerOnly: true },
  { id: "danger", label: "Danger zone", ownerOnly: true },
] as const;

export function SettingsView({ org, ws }: { org: string; ws: string }) {
  const router = useRouter();
  const resolved = useResolve(org, ws);
  const wsId = resolved.data?.workspace?.id;
  const workspace = useWorkspace(wsId);
  const [tab, setTab] = useState<(typeof TABS)[number]["id"]>("general");

  if (resolved.isPending || (wsId && workspace.isPending)) return <Loading />;
  if (resolved.isError) return <ErrorState error={resolved.error} onRetry={() => resolved.refetch()} />;
  if (!wsId) return <ErrorState error={{ code: "not_found" }} />;
  if (workspace.isError) return <ErrorState error={workspace.error} onRetry={() => workspace.refetch()} />;

  const w = workspace.data!;
  const canOwn = w.my_role === "owner";
  const tabs = TABS.filter((t) => canOwn || !t.ownerOnly);

  return (
    <div className="max-w-4xl space-y-4">
      <h1 className="text-xl font-semibold">Settings</h1>
      <div role="tablist" aria-label="Workspace settings" className="flex gap-1 border-b border-neutral-200">
        {tabs.map((t) => (
          <button
            key={t.id}
            role="tab"
            type="button"
            id={`tab-${t.id}`}
            aria-selected={tab === t.id}
            aria-controls="settings-panel"
            onClick={() => setTab(t.id)}
            className={cn(
              "-mb-px border-b-2 px-3 py-2 text-sm",
              tab === t.id ? "border-neutral-900 font-medium" : "border-transparent",
            )}
          >
            {t.label}
          </button>
        ))}
      </div>
      <div role="tabpanel" id="settings-panel" aria-labelledby={`tab-${tab}`} className="pt-2">
        {tab === "general" ? <GeneralTab key={w.updated_at} workspace={w} canOwn={canOwn} /> : null}
        {tab === "access" && canOwn ? <AccessTab wsId={w.id} orgId={w.org_id} canOwn={canOwn} /> : null}
        {tab === "keys" && canOwn ? <KeysTab wsId={w.id} /> : null}
        {tab === "audit" && canOwn ? <AuditTab wsId={w.id} /> : null}
        {tab === "danger" && canOwn ? (
          <div className="space-y-8">
            <WorkspaceFreeze wsId={w.id} name={w.name} orgId={w.org_id} frozen={w.frozen} />
            <DangerTab wsId={w.id} slug={w.slug} onDeleted={() => router.replace(`/o/${org}`)} />
          </div>
        ) : null}
      </div>
    </div>
  );
}
