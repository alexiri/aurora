"use client";

import { useRouter } from "next/navigation";
import { useState } from "react";

import { WorkspaceFreeze } from "@/components/freeze/freeze-sections";
import { ErrorState, Loading } from "@/components/states";
import { PageHeader } from "@/components/ui/page-header";
import { Tabs, tabId, tabPanelId } from "@/components/ui/tabs";
import { useResolve, useWorkspace } from "@/lib/api/queries";

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
    <div className="max-w-page space-y-4">
      <PageHeader
        title="Settings"
        description="Name, access, API keys and audit trail for this workspace."
        className="mb-2"
      />
      <Tabs
        items={tabs.map((t) => ({ value: t.id, label: t.label }))}
        value={tab}
        onValueChange={(v) => setTab(v as (typeof TABS)[number]["id"])}
        variant="underline"
        label="Workspace settings"
        idBase="ws-settings"
      />
      <div
        role="tabpanel"
        id={tabPanelId("ws-settings", tab)}
        aria-labelledby={tabId("ws-settings", tab)}
        className="pt-2"
      >
        {tab === "general" ? <GeneralTab key={w.updated_at} workspace={w} canOwn={canOwn} /> : null}
        {tab === "access" && canOwn ? <AccessTab wsId={w.id} orgId={w.org_id} canOwn={canOwn} /> : null}
        {tab === "keys" && canOwn ? <KeysTab wsId={w.id} /> : null}
        {tab === "audit" && canOwn ? <AuditTab wsId={w.id} /> : null}
        {tab === "danger" && canOwn ? (
          <div className="space-y-6">
            <WorkspaceFreeze wsId={w.id} name={w.name} orgId={w.org_id} frozen={w.frozen} />
            <DangerTab wsId={w.id} slug={w.slug} onDeleted={() => router.replace(`/o/${org}`)} />
          </div>
        ) : null}
      </div>
    </div>
  );
}
