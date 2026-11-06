"use client";

import { useState } from "react";

import { EmptyState, ErrorState, Loading } from "@/components/states";
import { cn } from "@/lib/utils";
import { useSecrets, type SecretScope } from "@/lib/api/secrets";

import { AppsTab } from "@/components/apps/apps-tab";
import { ConnectionsTab } from "@/components/apps/connections-tab";
import { OAuthClientsPanel } from "@/components/apps/oauth-clients-panel";
import { GenericSecretForm } from "./generic-form";
import { LlmKeyForm } from "./llm-form";
import { ProposalsList } from "./proposals";
import { SecretRow } from "./secret-row";

type Tab = "secrets" | "llm" | "proposals" | "apps" | "connections" | "oauth";

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
  const [tab, setTab] = useState<Tab>("secrets");
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
      <ul className="space-y-2" aria-label={kind === "llm" ? "LLM keys" : "Secrets"}>
        {rows.map((s) => (
          <SecretRow key={s.id} scope={scope} secret={s} canOwn={canOwn} />
        ))}
      </ul>
    );
  };

  return (
    <div className="max-w-4xl space-y-4">
      <h1 className="text-xl font-semibold">{title}</h1>
      <div role="tablist" aria-label="Credentials" className="flex gap-1 border-b border-neutral-200">
        {tabs.map((t) => (
          <button
            key={t.id}
            role="tab"
            type="button"
            aria-selected={tab === t.id}
            onClick={() => setTab(t.id)}
            className={cn("px-3 py-2 text-sm", tab === t.id ? "border-b-2 border-neutral-900 font-medium" : "text-neutral-600")}
          >
            {t.label}
          </button>
        ))}
      </div>
      <div role="tabpanel" className="space-y-4">
        {tab === "secrets" ? (
          <>
            {canOwn ? (
              adding ? (
                <GenericSecretForm scope={scope} onDone={() => setAdding(false)} />
              ) : (
                <button type="button" className="text-sm underline" onClick={() => setAdding(true)}>
                  Add a secret
                </button>
              )
            ) : (
              <p className="text-sm text-neutral-600">You can view these credentials but not change them.</p>
            )}
            {list("generic")}
          </>
        ) : null}
        {tab === "llm" ? (
          <>
            {canOwn ? <LlmKeyForm scope={scope} orgId={orgId} /> : <p className="text-sm text-neutral-600">You can view these keys but not change them.</p>}
            {list("llm")}
          </>
        ) : null}
        {tab === "apps" && scope.kind === "workspace" ? <AppsTab workspaceId={scope.id} canOwn={canOwn} /> : null}
        {tab === "connections" ? <ConnectionsTab scope={scope} canOwn={canOwn} /> : null}
        {tab === "oauth" ? <OAuthClientsPanel scope={scope} canOwn={canOwn} /> : null}
        {tab === "proposals" ? (
          secrets.isPending ? <Loading /> : secrets.isError ? <ErrorState error={secrets.error} /> : <ProposalsList scope={scope} items={items} canOwn={canOwn} />
        ) : null}
      </div>
    </div>
  );
}
