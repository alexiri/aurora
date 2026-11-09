"use client";

import { useState } from "react";

import { EmptyState, ErrorState, Loading } from "@/components/states";
import { Button } from "@/components/ui/button";
import { Notice } from "@/components/ui/notice";
import { PageHeader } from "@/components/ui/page-header";
import { Tabs, tabId, tabPanelId } from "@/components/ui/tabs";
import { useSecrets, type SecretScope } from "@/lib/api/secrets";
import { useTabParam } from "@/lib/use-tab-param";

import { AppsTab } from "@/components/apps/apps-tab";
import { ConnectionsTab } from "@/components/apps/connections-tab";
import { OAuthClientsPanel } from "@/components/apps/oauth-clients-panel";
import { GenericSecretForm } from "./generic-form";
import { LlmKeyForm } from "./llm-form";
import { ProposalsList } from "./proposals";
import { SecretRow } from "./secret-row";

type Tab = "secrets" | "llm" | "proposals" | "apps" | "connections" | "oauth";
const TABS: readonly Tab[] = ["secrets", "llm", "proposals", "apps", "connections", "oauth"];

export function CredentialsView({
  scope,
  orgId,
  canOwn,
  title,
}: {
  scope: SecretScope;
  orgId: string;
  canOwn: boolean;
  title: string;
}) {
  // `?tab=` selects the tab (C-506); an unknown value, or Proposals/Apps on an org page, falls back to Secrets.
  const [requested, setTab] = useTabParam<Tab>(TABS, "secrets");
  const tab: Tab = requested === "proposals" || requested === "apps" ? (scope.kind === "workspace" ? requested : "secrets") : requested;
  const [adding, setAdding] = useState(false);
  const secrets = useSecrets(scope);
  const items = secrets.data?.items ?? [];
  const tabs: { id: Tab; label: string }[] = [
    { id: "secrets", label: "Secrets" },
    { id: "llm", label: "LLM keys" },
    ...(scope.kind === "workspace" ? [{ id: "proposals" as Tab, label: "Proposals" }, { id: "apps" as Tab, label: "Apps" }] : []),
    { id: "connections", label: "Connections" },
    { id: "oauth", label: "OAuth clients" },
  ];

  const list = (kind: "generic" | "llm") => {
    if (secrets.isPending) return <Loading />;
    if (secrets.isError) return <ErrorState error={secrets.error} onRetry={() => secrets.refetch()} />;
    const rows = items.filter((s) => s.kind === kind && s.status === "active");
    if (rows.length === 0)
      return <EmptyState title={kind === "llm" ? "No LLM keys yet" : "No secrets yet"} />;
    return (
      <ul className="space-y-3" aria-label={kind === "llm" ? "LLM keys" : "Secrets"}>
        {rows.map((s) => (
          <SecretRow key={s.id} scope={scope} orgId={orgId} secret={s} canOwn={canOwn} />
        ))}
      </ul>
    );
  };

  return (
    <div className="max-w-[1160px] space-y-4">
      <PageHeader
        title={title}
        description="Secrets, LLM keys and app connections that agents use through the gateway."
        className="mb-2"
      />
      <Tabs
        items={tabs.map((t) => ({ value: t.id, label: t.label }))}
        value={tab}
        onValueChange={(v) => setTab(v as Tab)}
        variant="underline"
        label="Credentials"
        idBase="creds"
      />
      <div role="tabpanel" id={tabPanelId("creds", tab)} aria-labelledby={tabId("creds", tab)} className="space-y-4 pt-2">
        {tab === "secrets" ? (
          <>
            {canOwn ? (
              adding ? (
                <GenericSecretForm scope={scope} orgId={orgId} onDone={() => setAdding(false)} />
              ) : (
                <div>
                  <Button variant="outline" onClick={() => setAdding(true)}>
                    Add a secret
                  </Button>
                </div>
              )
            ) : (
              <Notice role="status">You can view these credentials but not change them.</Notice>
            )}
            {list("generic")}
          </>
        ) : null}
        {tab === "llm" ? (
          <>
            {canOwn ? <LlmKeyForm scope={scope} orgId={orgId} /> : <Notice role="status">You can view these keys but not change them.</Notice>}
            {list("llm")}
          </>
        ) : null}
        {tab === "apps" && scope.kind === "workspace" ? <AppsTab workspaceId={scope.id} canOwn={canOwn} /> : null}
        {tab === "connections" ? <ConnectionsTab scope={scope} canOwn={canOwn} /> : null}
        {tab === "oauth" ? <OAuthClientsPanel scope={scope} canOwn={canOwn} orgId={orgId} /> : null}
        {tab === "proposals" ? (
          secrets.isPending ? <Loading /> : secrets.isError ? <ErrorState error={secrets.error} /> : <ProposalsList scope={scope} items={items} canOwn={canOwn} />
        ) : null}
      </div>
    </div>
  );
}
