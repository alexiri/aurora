"use client";

import { useQueryClient } from "@tanstack/react-query";
import { useCallback, useState, type ReactNode } from "react";

import { ConnectDialog } from "@/components/apps/connect-dialog";
import { ConnectJourney } from "@/components/apps/connect-journey/connect-journey";
import { useUrlDialog } from "@/components/apps/use-url-dialog";
import { Notice } from "@/components/ui/notice";
import { appKeys, useConnectChannel } from "@/lib/api/apps";
import { LlmKeyForm } from "@/components/credentials/llm-form";
import { EmptyState, ErrorState, Loading } from "@/components/states";
import { Button } from "@/components/ui/button";
import { Menu, MenuContent, MenuItem, MenuTrigger } from "@/components/ui/menu";
import { accountKeys, useAccounts, type Account } from "@/lib/api/accounts";
import type { SecretScope } from "@/lib/api/secrets";
import { useWhoCan } from "@/lib/api/who-can";
import { splitGiven } from "@/lib/accounts";

import { AccountRow } from "./account-row";
import { FreeApps } from "./free-apps";

/**
 * The one list of what agents can use (C-925, `14 §1`), over the accounts API (C-917). Used by a workspace's Accounts
 * tab, an agent's Access tab (`agent`) and an organization's Accounts tab. `banner` is the slot above the list for the
 * requests banner (P5-UX-17). `kind` shows one heading's accounts only (`?kind=` of the tab).
 */
export function AccountsList({
  scope,
  orgId,
  canOwn,
  agent,
  banner,
  kind,
}: {
  scope: SecretScope;
  orgId: string;
  canOwn: boolean;
  /** The agent whose Access tab this is: its accounts come first, the others are collapsed under "Not given to …". */
  agent?: { id: string; name: string };
  banner?: ReactNode;
  kind?: "app" | "website" | "ai_key" | null;
}) {
  const accounts = useAccounts(scope.kind === "workspace" ? { kind: "workspace", id: scope.id, agentId: agent?.id } : { kind: "organization", id: scope.id });
  const [addingKey, setAddingKey] = useState(false);
  // The Reconnect dialog (D-272, C-842): opened by a row's menu or the deep link `?reconnect=<connection id>`.
  const [reconnecting, setReconnecting] = useUrlDialog("reconnect");
  // The connect journey (C-914, `14 §3`): **Give <Agent> an app**, **Connect an app or website**, or the deep link
  // `?tab=access&connect=<app>` ("" opens it at the App step).
  const [connecting, setConnecting] = useUrlDialog("connect");
  const qc = useQueryClient();
  const refresh = useCallback(() => {
    void qc.invalidateQueries({ queryKey: accountKeys.all });
    void qc.invalidateQueries({ queryKey: appKeys.connections(scope) });
  }, [qc, scope]);
  useConnectChannel(refresh);
  const who = useWhoCan(
    scope.kind === "workspace" ? { workspaceId: scope.id } : { orgId: scope.id },
    scope.kind === "workspace" ? "workspace_owner" : "org_admin",
    "add_account",
  );

  if (accounts.isPending) return <Loading />;
  if (accounts.isError) return <ErrorState error={accounts.error} onRetry={() => accounts.refetch()} />;
  const items = accounts.data.items.filter((a) => !kind || a.kind === kind);
  const canReconnect = (a: Account) => canOwn && scope.kind === "workspace" && a.ref.type === "connection" && a.scope === "workspace";
  const row = (a: Account) => (
    <AccountRow
      key={a.ref.id}
      account={a}
      scope={scope}
      orgId={orgId}
      workspaceId={scope.kind === "workspace" ? scope.id : undefined}
      agent={agent}
      onReconnect={canReconnect(a) ? () => setReconnecting(a.ref.id) : undefined}
    />
  );
  const target = reconnecting ? items.find((a) => a.ref.id === reconnecting && a.ref.type === "connection") : undefined;
  const reconnectOk = target ? canReconnect(target) : false;

  const addEntries = agent ? (
    <>
      {canOwn ? (
        <Button size="sm" onClick={() => setConnecting("")}>
          Give {agent.name} an app
        </Button>
      ) : null}
      {canOwn ? (
        <Button variant="outline" size="sm" onClick={() => setAddingKey(true)}>
          Add an AI key
        </Button>
      ) : null}
    </>
  ) : canOwn ? (
    <Menu>
      <MenuTrigger asChild>
        <Button size="sm">Add</Button>
      </MenuTrigger>
      <MenuContent>
        {scope.kind === "workspace" ? (
          <MenuItem onSelect={() => setConnecting("")}>Connect an app or website</MenuItem>
        ) : null}
        <MenuItem onSelect={() => setAddingKey(true)}>Add an AI key</MenuItem>
      </MenuContent>
    </Menu>
  ) : null;

  const { given, notGiven } = agent ? splitGiven(items) : { given: items, notGiven: [] as Account[] };
  return (
    <div className="space-y-4">
      {banner}
      {reconnecting && !reconnectOk ? (
        <Notice tone="pending" role="status">
          {!target
            ? "That account no longer exists, or it is not in this workspace."
            : !canOwn
              ? `Only a workspace owner can reconnect ${target.name}.`
              : `${target.name} cannot be reconnected here.`}
        </Notice>
      ) : null}
      {addEntries ? <div className="flex flex-wrap items-center justify-end gap-2">{addEntries}</div> : null}
      {addingKey ? (
        <div className="rounded-card border border-line bg-surface p-4">
          <LlmKeyForm scope={scope} orgId={orgId} preselect={agent ? [agent.id] : undefined} />
          <Button className="mt-3" variant="outline" size="sm" onClick={() => setAddingKey(false)}>
            Close
          </Button>
        </div>
      ) : null}
      {items.length === 0 ? (
        scope.kind === "workspace" ? (
          agent ? (
            <EmptyState
              title="Nothing is given yet"
              when={`${agent.name} cannot use any app, website or AI key yet.`}
              action={
                canOwn ? (
                  <Button size="sm" onClick={() => setConnecting("")}>
                    Give {agent.name} an app
                  </Button>
                ) : undefined
              }
              ask={{ whoCan: who, canAct: canOwn }}
            />
          ) : (
            <EmptyState
              title="Nothing connected yet"
              when="Connect an app, a website or an AI key, and choose which agents can use it."
              action={
                canOwn ? (
                  <Button size="sm" onClick={() => setConnecting("")}>
                    Connect an app or website
                  </Button>
                ) : undefined
              }
              ask={{ whoCan: who, canAct: canOwn }}
            />
          )
        ) : (
          <EmptyState
            title="No accounts for the whole organization yet"
            when="An account connected here can be given to agents in any workspace."
            ask={{ whoCan: who, canAct: canOwn }}
          />
        )
      ) : (
        <>
          {given.length > 0 ? (
            <ul className="space-y-3" aria-label={agent ? `Accounts ${agent.name} can use` : "Accounts"}>
              {given.map(row)}
            </ul>
          ) : null}
          {agent && notGiven.length > 0 ? (
            <details className="space-y-3">
              <summary className="cursor-pointer text-sm font-medium text-ink">Not given to {agent.name} ({notGiven.length})</summary>
              <ul className="mt-3 space-y-3" aria-label={`Not given to ${agent.name}`}>
                {notGiven.map(row)}
              </ul>
            </details>
          ) : null}
        </>
      )}
      {agent && canOwn && scope.kind === "workspace" ? <FreeApps agent={agent} workspaceId={scope.id} accounts={accounts.data.items} /> : null}
      {connecting !== null && canOwn && scope.kind === "workspace" ? (
        <ConnectJourney
          workspaceId={scope.id}
          orgId={orgId}
          agent={agent}
          appId={connecting || null}
          onClose={() => {
            setConnecting(null);
            refresh();
          }}
        />
      ) : null}
      {target && reconnectOk && target.app && scope.kind === "workspace" ? (
        <ConnectDialog
          appId={target.app.app_id}
          name={target.app.name}
          workspaceId={scope.id}
          orgId={orgId}
          reconnect={target.ref.id}
          onClose={() => {
            setReconnecting(null);
            refresh();
          }}
        />
      ) : null}
    </div>
  );
}
