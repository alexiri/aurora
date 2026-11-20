"use client";

import { useQueryClient } from "@tanstack/react-query";
import { useCallback, useState, type ReactNode } from "react";

import { ConnectDialog } from "@/components/apps/connect-dialog";
import { ConnectJourney } from "@/components/apps/connect-journey/connect-journey";
import { OrgConnectJourney } from "@/components/apps/connect-journey/org-journey";
import { useUrlDialog } from "@/components/apps/use-url-dialog";
import { Notice } from "@/components/ui/notice";
import { appKeys, useConnectChannel } from "@/lib/api/apps";
import { LlmKeyForm, useFooterSlot } from "@/components/credentials/llm-form";
import { EmptyState, ErrorState, Loading } from "@/components/states";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent } from "@/components/ui/dialog";
import { Card } from "@/components/ui/card";
import { Menu, MenuContent, MenuItem, MenuTrigger } from "@/components/ui/menu";
import { accountKeys, useAccounts, type Account } from "@/lib/api/accounts";
import type { SecretScope } from "@/lib/api/secrets";
import { useWhoCan } from "@/lib/api/who-can";
import { useOrg } from "@/lib/api/queries";
import { splitGiven } from "@/lib/accounts";
import { needsAttention } from "@/lib/account-health";

import { BulkSelectAll, useBulkOn, useSelection } from "@/components/ui/bulk-bar";
import { AccountRow } from "./account-row";
import { AccountsBulkBar } from "./accounts-bulk";
import { FreeApps } from "./free-apps";

/** "3 accounts." / "1 account needs you." / "2 accounts need you." */
export function accountsSentence(total: number, failing: number, soleAgent?: string): string {
  if (failing > 0) return failing === 1 ? "1 account needs you." : `${failing} accounts need you.`;
  const count = total === 1 ? "1 account." : `${total} accounts.`;
  // (D1) "2 accounts. Daneel uses both." when one agent uses all of them.
  return soleAgent ? `${count} ${soleAgent} uses ${total === 1 ? "it" : total === 2 ? "both" : "all of them"}.` : count;
}

/** The one agent that uses every account of the list, or `undefined` (none, several, or an account nobody uses). */
export function soleAgentOf(items: Pick<Account, "agents">[]): string | undefined {
  const names = new Set<string>();
  for (const a of items) {
    if (a.agents.length === 0) return undefined;
    for (const e of a.agents) names.add(e.agent.id);
  }
  if (names.size !== 1) return undefined;
  return items[0]?.agents[0]?.agent.name;
}

/** (P5-POLISH-11, A1) Add an AI key: the form without its own card, Cancel and Add key in the dialog's footer row. */
function AddKeyDialog({ scope, orgId, agentId, onClose }: { scope: SecretScope; orgId: string; agentId?: string; onClose: () => void }) {
  const { footer, target } = useFooterSlot();
  return (
    <Dialog open onOpenChange={(o) => (o ? null : onClose())}>
      <DialogContent size="lg" title="Add an AI key" footer={footer}>
        <LlmKeyForm scope={scope} orgId={orgId} preselect={agentId ? [agentId] : undefined} submitLabel="Add key" onCancel={onClose} footerTarget={target} />
      </DialogContent>
    </Dialog>
  );
}

/** (P5-POLISH-9, E2) The organization's Accounts: "1 account, shared with everyone in Users." (an account that needs you comes first). */
export function orgAccountsSentence(total: number, failing: number, orgName: string | undefined): string {
  if (failing > 0) return accountsSentence(total, failing);
  const who = `everyone in ${orgName ?? "the organization"}`;
  return total === 1 ? `1 account, shared with ${who}.` : `${total} accounts, shared with ${who}.`;
}

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
  appId,
}: {
  scope: SecretScope;
  orgId: string;
  canOwn: boolean;
  /** (P5-UX-9) Only this app's accounts, for the app's setup page: no Add, and an empty list says so in one line. */
  appId?: string;
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

  const org = useOrg(scope.kind === "organization" ? orgId : undefined);
  // (P5-REFINE-19, Expert) Bulk on the main list: a checkbox per account the viewer manages or may give, a bar above the list.
  const bulkOn = useBulkOn();
  const bulkable = !agent && !appId;
  const sel = useSelection(
    bulkable
      ? (accounts.data?.items ?? []).filter((a) => (!kind || a.kind === kind) && (a.can.manage || a.can.give_access)).map((a) => a.ref.id)
      : [],
  );
  if (accounts.isPending) return <Loading variant="rows" label="Loading your accounts…" />;
  if (accounts.isError) return <ErrorState error={accounts.error} onRetry={() => accounts.refetch()} />;
  const items = accounts.data.items.filter((a) => (!kind || a.kind === kind) && (!appId || a.app?.app_id === appId));
  // A workspace owner reconnects the workspace's own accounts; an organization owner or admin the organization's (C-937).
  const canReconnect = (a: Account) =>
    canOwn && a.ref.type === "connection" && (scope.kind === "workspace" ? a.scope === "workspace" : a.scope === "organization");
  const row = (a: Account) => (
    <AccountRow
      key={a.ref.id}
      account={a}
      scope={scope}
      orgId={orgId}
      workspaceId={scope.kind === "workspace" ? scope.id : undefined}
      agent={agent}
      onReconnect={canReconnect(a) ? () => setReconnecting(a.ref.id) : undefined}
      select={bulkable && (a.can.manage || a.can.give_access) ? { checked: sel.selected.has(a.ref.id), onChange: (on) => sel.toggle(a.ref.id, on), disabled: false } : undefined}
    />
  );
  const picked = items.filter((a) => sel.selected.has(a.ref.id));
  const target = reconnecting ? items.find((a) => a.ref.id === reconnecting && a.ref.type === "connection") : undefined;
  const reconnectOk = target ? canReconnect(target) : false;

  const addEntries = appId ? null : agent ? (
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
      <MenuContent align="end">
        <MenuItem onSelect={() => setConnecting("")}>Connect an app or website</MenuItem>
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
      {/* (P5-POLISH-7, D1) A sentence from state; an account that needs you takes precedence. */}
      {/* The sentence and **Add** share one line (the mockup); a phone wraps Add below. */}
      {!agent && !appId && items.length > 0 ? (
        <div className="flex flex-wrap items-center justify-between gap-2">
          <p className="min-w-0 flex-1 text-sm text-ink-2" data-testid="accounts-sentence">
            {scope.kind === "organization"
              ? orgAccountsSentence(items.length, items.filter(needsAttention).length, org.data?.name)
              : accountsSentence(items.length, items.filter(needsAttention).length, soleAgentOf(items))}
          </p>
          {addEntries ? <div className="flex flex-wrap items-center gap-2">{addEntries}</div> : null}
        </div>
      ) : agent && addEntries ? (
        <div className="flex flex-wrap items-center justify-between gap-2">
          <h2 className="font-display text-base font-semibold text-ink">Apps and accounts</h2>
          <div className="flex flex-wrap items-center gap-2">{addEntries}</div>
        </div>
      ) : addEntries ? (
        <div className="flex flex-wrap items-center justify-end gap-2">{addEntries}</div>
      ) : null}
      {bulkable && bulkOn && items.some((a) => a.can.manage || a.can.give_access) ? (
        <label className="flex items-center text-sm text-ink-2">
          <BulkSelectAll label="Select all accounts" all={sel.all} some={sel.some} onChange={sel.setAll} />
          Select all
        </label>
      ) : null}
      {bulkable ? <AccountsBulkBar picked={picked} scope={scope} orgId={orgId} onClear={sel.clear} /> : null}
      {/* (P5-POLISH-7, D1) Add an AI key is a dialog like every other add, not a card inside the page. */}
      {addingKey ? <AddKeyDialog scope={scope} orgId={orgId} agentId={agent?.id} onClose={() => setAddingKey(false)} /> : null}
      {items.length === 0 && appId ? (
        <p className="text-sm text-ink-2">No accounts of this app {scope.kind === "workspace" ? "in this workspace" : "for the whole organization"} yet.</p>
      ) : items.length === 0 ? (
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
        <>
          {given.length > 0 ? (
            // (P5-POLISH-11, C1) On an agent's page the accounts are one bordered list with dividers.
            agent ? (
              <Card className="p-0 sm:p-0">
                <ul className="divide-y divide-line" aria-label={`Accounts ${agent.name} can use`}>
                  {given.map(row)}
                </ul>
              </Card>
            ) : (
              <ul className="space-y-3" aria-label="Accounts">
                {given.map(row)}
              </ul>
            )
          ) : null}
          {agent && notGiven.length > 0 ? (
            <details className="space-y-3">
              <summary className="cursor-pointer text-sm font-medium text-ink">Not given to {agent.name} ({notGiven.length})</summary>
              <Card className="mt-3 p-0 sm:p-0">
                <ul className="divide-y divide-line" aria-label={`Not given to ${agent.name}`}>
                  {notGiven.map(row)}
                </ul>
              </Card>
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
      {connecting !== null && canOwn && scope.kind === "organization" ? (
        <OrgConnectJourney
          orgId={scope.id}
          appId={connecting || null}
          onClose={() => {
            setConnecting(null);
            refresh();
          }}
        />
      ) : null}
      {target && reconnectOk && target.app ? (
        <ConnectDialog
          appId={target.app.app_id}
          name={target.app.name}
          workspaceId={scope.kind === "workspace" ? scope.id : ""}
          orgScope={scope.kind === "organization"}
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
