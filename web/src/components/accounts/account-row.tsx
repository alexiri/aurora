"use client";

import { useState } from "react";

import { AppIcon } from "@/components/ui/app-icon";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent } from "@/components/ui/dialog";
import { ErrorState } from "@/components/states";
import { WhoCan } from "@/components/ui/who-can";
import { type Account, type AgentAccess, useGiveSecret, usePutAccess, useRemoveAccess } from "@/lib/api/accounts";
import type { SecretScope } from "@/lib/api/secrets";
import { entryText, kindLabel } from "@/lib/accounts";

import { AccessControl } from "./access-control";
import { AccountHealth } from "./account-health";
import { AccountMenu } from "./account-menu";
import { ConnectionDetails } from "./connection-details";

/** One agent's entry in "who can use it": opens the access choice and **Remove access** (`14 §1`). */
function AgentEntry({ account, entry, workspaceId, canChange }: { account: Account; entry: AgentAccess; workspaceId: string; canChange: boolean }) {
  const [open, setOpen] = useState(false);
  const remove = useRemoveAccess();
  if (!canChange) return <li className="text-sm text-ink">{entryText(entry)}</li>;
  return (
    <li>
      <Button variant="outline" size="sm" onClick={() => setOpen(true)} aria-label={`${entryText(entry)}: change access`}>
        {entryText(entry)}
      </Button>
      <Dialog open={open} onOpenChange={setOpen}>
        <DialogContent size="lg" title={`${entry.agent.name} and ${account.name}`}>
          {open ? (
            <div className="space-y-4">
              <AccessControl account={account} entry={entry} workspaceId={workspaceId} canChange />
              {remove.isError ? <ErrorState error={remove.error} /> : null}
              <Button
                variant="outline"
                disabled={remove.isPending}
                onClick={() => remove.mutate({ agentId: entry.agent.id, account }, { onSuccess: () => setOpen(false) })}
              >
                Remove access
              </Button>
            </div>
          ) : null}
        </DialogContent>
      </Dialog>
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
  const own: SecretScope = account.scope === "organization" ? { kind: "organization", id: orgId } : scope;
  const mine = agent ? account.agents.find((a) => a.agent.id === agent.id) : undefined;
  const ws = workspaceId ?? (scope.kind === "workspace" ? scope.id : "");
  return (
    <li className="space-y-3 rounded-card border border-line bg-surface p-4" aria-label={account.name}>
      <div className="flex flex-wrap items-start gap-3">
        {account.app ? <AppIcon appId={account.app.app_id} name={account.app.name} size={32} /> : null}
        <div className="min-w-0 flex-1 space-y-1">
          <div className="flex flex-wrap items-center gap-2">
            <h3 className="min-w-0 break-words font-display text-base font-semibold text-ink">{account.name}</h3>
            <Badge>{kindLabel(account.kind)}</Badge>
            {shared ? <Badge tone="accent">Shared by your organization</Badge> : null}
          </div>
          {account.app && account.app.name !== account.name ? <p className="text-sm text-muted">{account.app.name}</p> : null}
          <AccountHealth account={account} reconnectHref={account.can.manage ? reconnectHref : undefined} />
          {shared && account.managed_by ? <WhoCan value={account.managed_by} /> : null}
        </div>
        {account.can.manage ? (
          <AccountMenu account={account} scope={own} orgId={orgId} contextAgentId={agent?.id} onReconnect={onReconnect} />
        ) : null}
      </div>
      {agent ? (
        <div className="space-y-2">
          {mine ? (
            <>
              <AccessControl account={account} entry={mine} workspaceId={ws} canChange={account.can.change_access} startOpen={false} />
              {mine.model ? <p className="text-sm text-ink-2">{agent.name} thinks with {mine.model} through this key.</p> : null}
              {account.levels.length === 0 && account.can.change_access ? <RemoveOnly account={account} agentId={agent.id} /> : null}
            </>
          ) : account.can.give_access ? (
            <GiveOne account={account} agent={agent} />
          ) : null}
        </div>
      ) : account.agents.length === 0 ? (
        <div className="flex flex-wrap items-center gap-2">
          <Badge tone="pending">No agent can use this</Badge>
        </div>
      ) : (
        <ul className="flex flex-wrap items-center gap-2" aria-label="Who can use it">
          {account.agents.map((a) => (
            <AgentEntry key={a.agent.id} account={account} entry={a} workspaceId={a.workspace.id} canChange={account.can.change_access} />
          ))}
        </ul>
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

function RemoveOnly({ account, agentId }: { account: Account; agentId: string }) {
  const remove = useRemoveAccess();
  return (
    <>
      {remove.isError ? <ErrorState error={remove.error} /> : null}
      <Button variant="outline" size="sm" disabled={remove.isPending} onClick={() => remove.mutate({ agentId, account })}>
        Remove access
      </Button>
    </>
  );
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
