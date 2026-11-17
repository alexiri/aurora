"use client";

import { useSearchParams } from "next/navigation";
import { useState } from "react";

import { EmptyState, ErrorState, Loading } from "@/components/states";
import { Button } from "@/components/ui/button";
import { MainAction } from "@/components/ui/main-action";
import { PageHeader } from "@/components/ui/page-header";
import { Section } from "@/components/ui/section";
import { Tabs, tabId, tabPanelId } from "@/components/ui/tabs";
import { useSecrets, type SecretScope } from "@/lib/api/secrets";
import { useWhoCan } from "@/lib/api/who-can";
import { useTabParam } from "@/lib/use-tab-param";

import { AccountsList } from "@/components/accounts/accounts-list";
import { AppsTab } from "@/components/apps/apps-tab";
import { ConnectionsTab } from "@/components/apps/connections-tab";
import { OAuthClientsPanel } from "@/components/apps/oauth-clients-panel";
import { GenericSecretForm } from "./generic-form";
import { LlmKeyForm } from "./llm-form";
import { ProposalsList } from "./proposals";
import { SecretRow } from "./secret-row";

type Tab = "secrets" | "llm" | "proposals" | "apps" | "connections" | "oauth";
const TABS: readonly Tab[] = ["secrets", "llm", "proposals", "apps", "connections", "oauth"];
/** The tab ids the page accepts (the dashboard-links conformance test checks the gateway's links against them). */
export const CREDENTIAL_TAB_IDS: readonly string[] = TABS;

/** The stored secrets of one kind (websites' tokens, or AI keys), as rows. */
function SecretsList({
  scope,
  orgId,
  canOwn,
  kind,
  secrets,
}: {
  scope: SecretScope;
  orgId: string;
  canOwn: boolean;
  kind: "generic" | "llm";
  secrets: ReturnType<typeof useSecrets>;
}) {
  // (C-916) Who adds an account here, for a viewer who cannot: the workspace's owners, or the organization's admins.
  const who = useWhoCan(
    scope.kind === "workspace" ? { workspaceId: scope.id } : { orgId: scope.id },
    scope.kind === "workspace" ? "workspace_owner" : "org_admin",
    "add_account",
  );
  if (secrets.isPending) return <Loading />;
  if (secrets.isError) return <ErrorState error={secrets.error} onRetry={() => secrets.refetch()} />;
  const rows = (secrets.data?.items ?? []).filter((s) => s.kind === kind && s.status === "active");
  if (rows.length === 0)
    return kind === "llm" ? (
      <EmptyState
        title="No AI keys yet"
        when="You need one before an agent can answer. Add it with the form above, then give your agent access to it."
        ask={{ whoCan: who, canAct: canOwn }}
      >
        An AI key is the password to the AI service that does your agents&apos; thinking.
      </EmptyState>
    ) : (
      <EmptyState
        title="No websites yet"
        when="Add one when an agent should reach a website that needs a login. The agent never sees the token: the gateway adds it."
        ask={{ whoCan: who, canAct: canOwn }}
      >
        A website here is any site or API Aurora holds a token for, so an agent can call it.
      </EmptyState>
    );
  return (
    <ul className="space-y-3" aria-label={kind === "llm" ? "AI keys" : "Websites"}>
      {rows.map((s) => (
        <SecretRow key={s.id} scope={scope} orgId={orgId} secret={s} canOwn={canOwn} />
      ))}
    </ul>
  );
}

/**
 * The Accounts tab of Apps & accounts (P5-IA-1): the existing lists under the headings Apps, Websites and AI keys.
 * P5-UX-4 replaces it with the one Accounts list. `kind=` (`app`, `website`, `ai-key`) shows one heading only; in a
 * workspace, agents' requests for an account come last (`#requests`).
 */
export function AccountsPanel({ scope, orgId, canOwn }: { scope: SecretScope; orgId: string; canOwn: boolean }) {
  const param = useSearchParams().get("kind");
  const kind = param === "app" || param === "website" ? param : param === "ai-key" ? "ai_key" : null;
  const secrets = useSecrets(scope, scope.kind === "workspace" && canOwn);
  // (P5-UX-4, C-917, C-925) One list for apps, websites and AI keys. The requests banner slot is P5-UX-17's; agents'
  // proposals of a secret stay below the list until then.
  return (
    <div className="space-y-8">
      <AccountsList scope={scope} orgId={orgId} canOwn={canOwn} kind={kind} />
      {scope.kind === "workspace" && canOwn && !kind ? (
        <Section id="requests" title="Requests" description="Accounts that agents asked for.">
          {secrets.isPending ? (
            <Loading />
          ) : secrets.isError ? (
            <ErrorState error={secrets.error} />
          ) : (
            <ProposalsList scope={scope} items={secrets.data?.items ?? []} canOwn={canOwn} />
          )}
        </Section>
      ) : null}
    </div>
  );
}

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
  // The connect / reconnect dialogs are addressed by `?connect=` / `?reconnect=` (C-842): another tab drops them.
  const [requested, setTab] = useTabParam<Tab>(TABS, "secrets", { drop: () => ["connect", "reconnect"] });
  const tab: Tab = requested === "proposals" || requested === "apps" ? (scope.kind === "workspace" ? requested : "secrets") : requested;
  const [adding, setAdding] = useState(false);
  const secrets = useSecrets(scope);
  const who = useWhoCan(
    scope.kind === "workspace" ? { workspaceId: scope.id } : { orgId: scope.id },
    scope.kind === "workspace" ? "workspace_owner" : "org_admin",
    "add_account",
  );
  const items = secrets.data?.items ?? [];
  const tabs: { id: Tab; label: string }[] = [
    { id: "secrets", label: "Secrets" },
    { id: "llm", label: "LLM keys" },
    ...(scope.kind === "workspace" ? [{ id: "proposals" as Tab, label: "Proposals" }, { id: "apps" as Tab, label: "Apps" }] : []),
    { id: "connections", label: "Connections" },
    { id: "oauth", label: "OAuth clients" },
  ];

  const list = (kind: "generic" | "llm") => (
    <SecretsList scope={scope} orgId={orgId} canOwn={canOwn} kind={kind} secrets={secrets} />
  );

  return (
    <div className="max-w-page space-y-4">
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
            <MainAction canAct={canOwn} whoCan={who}>
              {adding ? (
                <GenericSecretForm scope={scope} orgId={orgId} onDone={() => setAdding(false)} />
              ) : (
                <div>
                  <Button variant="outline" onClick={() => setAdding(true)}>
                    Add a secret
                  </Button>
                </div>
              )}
            </MainAction>
            {list("generic")}
          </>
        ) : null}
        {tab === "llm" ? (
          <>
            <MainAction canAct={canOwn} whoCan={who}>
              <LlmKeyForm scope={scope} orgId={orgId} />
            </MainAction>
            {list("llm")}
          </>
        ) : null}
        {tab === "apps" && scope.kind === "workspace" ? <AppsTab workspaceId={scope.id} orgId={orgId} canOwn={canOwn} /> : null}
        {tab === "connections" ? <ConnectionsTab scope={scope} canOwn={canOwn} /> : null}
        {tab === "oauth" ? <OAuthClientsPanel scope={scope} canOwn={canOwn} orgId={orgId} /> : null}
        {tab === "proposals" ? (
          secrets.isPending ? <Loading /> : secrets.isError ? <ErrorState error={secrets.error} /> : <ProposalsList scope={scope} items={items} canOwn={canOwn} />
        ) : null}
      </div>
    </div>
  );
}
