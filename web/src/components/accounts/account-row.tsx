"use client";

import { useState } from "react";

import { AppLink } from "@/components/apps/app-link";
import { AiKeyTile } from "@/components/ui/ai-key-tile";
import { AppIcon } from "@/components/ui/app-icon";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Avatar } from "@/components/agent-avatar";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { ErrorState } from "@/components/states";
import { WhoCan } from "@/components/ui/who-can";
import { useQueryClient } from "@tanstack/react-query";

import { accountKeys, type Account, type AgentAccess, useGiveSecret, usePutAccess, useRemoveAccess, useTestAccount } from "@/lib/api/accounts";
import { usePatchConnection } from "@/lib/api/apps";
import type { HealthFix } from "@/lib/account-health";
import type { SecretScope } from "@/lib/api/secrets";
import { useWhoCan } from "@/lib/api/who-can";
import { useMe, useOrg, useWorkspaces } from "@/lib/api/queries";
import { workspaceDisplayName } from "@/lib/workspace-name";
import { entryText, kindLabel } from "@/lib/accounts";
import { orList } from "@/lib/who-can";
import { WebhookCardLine } from "@/components/webhook/webhook-card-line";
import { LimitControl } from "./limit-control";

import { AccessControl } from "./access-control";
import { AccountHealth } from "./account-health";
import { AccountMenu } from "./account-menu";
import { ConnectionDetails } from "./connection-details";

/** One agent's entry in "who can use it": opens the access choice and **Remove access** (`14 §1`). */
/** "Bob's workspace" for an agent's workspace, read from the organization's workspace list (a personal one carries its owner). */
export function workspaceLabelOf(
  ws: { id: string; name: string },
  list: { id: string; name: string; personal_of?: { id: string; name: string } | null }[] | undefined,
  viewer: { id?: string } | null | undefined,
): string {
  const full = list?.find((w) => w.id === ws.id);
  return workspaceDisplayName(full ?? { name: ws.name }, viewer);
}

function AgentEntry({ account, entry, workspaceId, canChange, orgName, workspaceLabel }: { account: Account; entry: AgentAccess; workspaceId: string; canChange: boolean; orgName?: string; workspaceLabel?: string }) {
  // The pill's words; outside its own workspace the agent carries its workspace: "Daneel · Bob's workspace · Read only".
  const base = entryText(entry, account);
  const text = workspaceLabel ? base.replace(entry.agent.name, `${entry.agent.name} · ${workspaceLabel}`) : base;
  const [open, setOpen] = useState(false);
  const remove = useRemoveAccess();
  // (P5-POLISH-7, D1) A pill: the agent and its access level. For someone who may change it, pressing it opens the access
  // select in a popover (not a dialog); Remove access stays there for the accounts that have no levels.
  const pill = "inline-flex min-h-9 items-center gap-2 rounded-full border border-line bg-surface px-2.5 py-1 text-sm text-ink";
  const avatar = (
    <span aria-hidden="true" className="inline-flex">
      <Avatar name={entry.agent.name} tintKey={entry.agent.id} size={20} />
    </span>
  );
  if (!canChange) {
    return (
      <li className={pill}>
        {avatar}
        {text}
      </li>
    );
  }
  return (
    <li>
      <Popover open={open} onOpenChange={setOpen}>
        <PopoverTrigger asChild>
          <button type="button" className={`${pill} hover:bg-subtle`}>
            {avatar}
            {text}
          </button>
        </PopoverTrigger>
        <PopoverContent className="w-96 space-y-3">
          {open ? (
            <>
              <p className="text-sm font-medium text-ink">
                {entry.agent.name} and {account.name}
              </p>
              <AccessControl account={account} entry={entry} workspaceId={workspaceId} canChange orgName={orgName} />
              {remove.isError ? <ErrorState error={remove.error} /> : null}
              <Button
                variant="outline"
                size="sm"
                disabled={remove.isPending}
                onClick={() => remove.mutate({ agentId: entry.agent.id, account }, { onSuccess: () => setOpen(false) })}
              >
                Remove from {entry.agent.name}
              </Button>
            </>
          ) : null}
        </PopoverContent>
      </Popover>
    </li>
  );
}

/**
 * One account (`14 §1`): icon and name, a kind chip, who can use it, the health line, "Shared by your organization" and
 * the menu. With `agent` (an agent's Access tab) the row shows that agent's access in place, as `AccessControl`, and
 * an AI key its model line. People who cannot change an account see values as text and no controls.
 */
export function AccountRow({
  account,
  scope,
  orgId,
  workspaceId,
  agent,
  reconnectHref,
  onReconnect,
}: {
  account: Account;
  /** The scope this account lives in. */
  scope: SecretScope;
  orgId: string;
  workspaceId?: string;
  /** The agent whose Access tab this is. */
  agent?: { id: string; name: string };
  reconnectHref?: string;
  onReconnect?: () => void;
}) {
  const shared = account.scope === "organization" && scope.kind === "workspace";
  const org = useOrg(account.scope === "organization" ? orgId : undefined);
  const me = useMe();
  const orgWorkspaces = useWorkspaces(scope.kind === "organization" ? orgId : undefined);
  const own: SecretScope = account.scope === "organization" ? { kind: "organization", id: orgId } : scope;
  const mine = agent ? account.agents.find((a) => a.agent.id === agent.id) : undefined;
  const ws = workspaceId ?? (scope.kind === "workspace" ? scope.id : "");
  // Test (C-943): a result that is stored shows through the list's health; one that is not (an account that cannot be
  // tested, or a test that could not run) replaces the line until the next one.
  const test = useTestAccount();
  const patchConn = usePatchConnection({ kind: own.kind, id: own.id });
  const qc = useQueryClient();
  const [outcome, setOutcome] = useState<"not_testable" | "error" | null>(null);
  const [replacing, setReplacing] = useState(false);
  const runTest = () => {
    setOutcome(null);
    test.mutate(
      { ref: account.ref },
      { onSuccess: (a) => setOutcome(a.result === "not_testable" ? "not_testable" : null), onError: () => setOutcome("error") },
    );
  };
  const fix = (kind: HealthFix) => {
    if (kind === "test") runTest();
    else if (kind === "turn_on")
      patchConn.mutate({ id: account.ref.id, status: "active" }, { onSuccess: () => void qc.invalidateQueries({ queryKey: accountKeys.all }) });
    else if (kind === "replace" && account.ref.type === "secret") setReplacing(true);
    // A connection's token, password or key is replaced in the same dialog as a reconnect (C-842).
    else onReconnect?.();
  };
  const canFix = (kind: HealthFix) => account.can.manage && (kind === "test" || kind === "turn_on" || kind === "replace" && account.ref.type === "secret" || !!onReconnect);
  return (
    // On an agent's page the rows share one Card with dividers (the list owns the frame); elsewhere each is a card.
    <li className={agent ? "space-y-3 p-4" : "space-y-3 rounded-card border border-line bg-surface p-4"} aria-label={account.name}>
      <div className="flex flex-wrap items-start gap-3">
        {account.app ? (
          <AppIcon appId={account.app.app_id} name={account.app.name} size={32} />
        ) : account.kind === "ai_key" ? (
          <AiKeyTile />
        ) : null}
        <div className="min-w-0 flex-1 space-y-1">
          <div className="flex flex-wrap items-center gap-2">
            <h3 className="min-w-0 break-words font-display text-base font-semibold text-ink">
              {account.app ? (
                <>
                  {/* (C-933) The setup page is for people who can change things; a member reads the name. */}
                  {account.can.manage || account.can.give_access ? (
                    <AppLink app={{ app_id: account.app.app_id }} className="font-display text-base font-semibold">
                      {account.app.name}
                    </AppLink>
                  ) : (
                    account.app.name
                  )}
                  {account.app.name !== account.name ? ` · ${account.name}` : null}
                </>
              ) : (
                account.name
              )}
            </h3>
            {/* (P5-POLISH-11, F2) A kind is its icon: apps and AI keys carry a tile, no chip; only a website has no tile. */}
            {/* (P5-POLISH-9, E2) An organization's account says what it is by its name, not a kind chip. */}
            {account.kind === "website" && !agent && scope.kind !== "organization" ? <Badge>{kindLabel(account.kind)}</Badge> : null}
          </div>
          {/* (D1) The sharing is one line, not a chip: "Shared by your organization · managed by alice". */}
          {shared && !agent ? (
            <p className="text-sm text-ink-2">
              Shared by your organization
              {account.managed_by && account.managed_by.people.length > 0 ? ` · managed by ${orList(account.managed_by.people.map((p) => p.name))}` : ""}
            </p>
          ) : null}
          <AccountHealth
            account={account}
            outcome={test.isPending ? null : outcome}
            onFix={account.can.manage ? (k) => (canFix(k) ? fix(k) : undefined) : undefined}
          />
          {account.status_reason === "instance_deleted" && account.health.state === "disabled" ? <InstanceRemoved orgId={orgId} /> : null}
          {test.isPending ? <p className="text-sm text-muted" role="status">Testing…</p> : null}
          {/* (P5-WEBHOOK-4, C-913) An Outgoing webhook: its host and limit, Edit and Send test, for those who manage it. */}
          {account.app?.app_id === "webhook" && account.ref.type === "connection" && account.can.manage ? (
            <WebhookCardLine connectionId={account.ref.id} name={account.name} />
          ) : null}
          {/* No who-can line on an agent's page (C1: who manages it is on the account page); in the list only when no one is named. */}
          {shared && !agent && account.managed_by && account.managed_by.people.length === 0 ? <WhoCan value={account.managed_by} /> : null}
          {/* (P5-UX-11, C-937) The limit of the organization's account, for the people who manage it. */}
          {scope.kind === "organization" && account.scope === "organization" && account.can.manage ? <LimitControl account={account} orgId={orgId} /> : null}
        </div>
        {/* (C1) The agent's level sits in the right cluster just before the … button; its details wrap below. */}
        {agent && mine ? (
          <AccessControl account={account} entry={mine} workspaceId={ws} canChange={account.can.change_access} startOpen={false} orgName={org.data?.name} inRow />
        ) : null}
        {account.can.manage ? (
          <AccountMenu
            account={account}
            scope={own}
            orgId={orgId}
            contextAgentId={agent?.id}
            agent={agent}
            onReconnect={onReconnect}
            onTest={runTest}
            replacing={replacing}
            onReplacingChange={setReplacing}
          />
        ) : null}
      </div>
      {agent ? (
        <div className="space-y-2">
          {mine ? (
            <>
              {mine.model ? (
                <p className="text-sm text-ink-2">
                  {shared ? "Shared by your organization · " : ""}
                  {agent.name} thinks with {mine.model}
                  {shared ? "" : " through this key"}
                </p>
              ) : shared ? (
                <p className="text-sm text-ink-2">Shared by your organization</p>
              ) : null}
            </>
          ) : account.can.give_access ? (
            <>
              {shared ? <p className="text-sm text-ink-2">Shared by your organization</p> : null}
              <GiveOne account={account} agent={agent} />
            </>
          ) : null}
        </div>
      ) : account.agents.length === 0 ? (
        <div className="flex flex-wrap items-center gap-2">
          <Badge>No agent can use this</Badge>
        </div>
      ) : (
        // (P5-POLISH-9, E2) An agent outside its own workspace is "<Agent> · <workspace>"; more than three agents fold into
        // "Used by <n> agents", which opens the pills.
        (() => {
          const pills = (
            <ul className="flex flex-wrap items-center gap-2" aria-label="Who can use it">
              {account.agents.map((a) => (
                <AgentEntry
                  key={a.agent.id}
                  account={account}
                  entry={a}
                  workspaceId={a.workspace.id}
                  canChange={account.can.change_access}
                  orgName={org.data?.name}
                  workspaceLabel={scope.kind === "organization" ? workspaceLabelOf(a.workspace, orgWorkspaces.data?.items, me.data?.user) : undefined}
                />
              ))}
            </ul>
          );
          return account.agents.length > 3 ? (
            <details>
              <summary className="cursor-pointer text-sm font-medium text-ink">Used by {account.agents.length} agents</summary>
              <div className="mt-2">{pills}</div>
            </details>
          ) : (
            pills
          );
        })()
      )}
      {account.ref.type === "connection" ? (
        <details className="text-sm">
          <summary className="cursor-pointer text-muted">Technical details</summary>
          <div className="mt-2">
            <ConnectionDetails scope={own} id={account.ref.id} canManage={account.can.manage} />
          </div>
        </details>
      ) : null}
    </li>
  );
}

/** (FIX-163) Who can add the removed app instance again: its registration is for organization owners and admins. */
function InstanceRemoved({ orgId }: { orgId: string }) {
  const who = useWhoCan({ orgId }, "org_admin", "add_instance");
  return <WhoCan value={who} lead />;
}

/** An account the agent cannot use yet, on its Access tab: **Give <Agent> access** (read only for an app; `use` otherwise). */
function GiveOne({ account, agent }: { account: Account; agent: { id: string; name: string } }) {
  return <GiveAccountToAgent account={account} agent={agent} />;
}

function GiveAccountToAgent({ account, agent }: { account: Account; agent: { id: string; name: string } }) {
  const putAccess = usePutAccess();
  const giveSecret = useGiveSecret();
  const busy = putAccess.isPending || giveSecret.isPending;
  const error = putAccess.error ?? giveSecret.error;
  return (
    <div className="space-y-2">
      {error ? <ErrorState error={error} /> : null}
      <Button
        variant="outline"
        size="sm"
        disabled={busy}
        onClick={() =>
          account.ref.type === "secret"
            ? giveSecret.mutate({ agentId: agent.id, secretId: account.ref.id })
            : putAccess.mutate({ agentId: agent.id, connectionId: account.ref.id, body: { level: "read" } })
        }
      >
        Give {agent.name} access
      </Button>
    </div>
  );
}
