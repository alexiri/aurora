"use client";

import Link from "next/link";
import { useState, type ReactNode } from "react";

import { LlmKeyForm } from "@/components/credentials/llm-form";
import { EmptyState, ErrorState, Loading } from "@/components/states";
import { Button } from "@/components/ui/button";
import { buttonVariants } from "@/components/ui/button";
import { Menu, MenuContent, MenuItem, MenuTrigger } from "@/components/ui/menu";
import { useAccounts, type Account } from "@/lib/api/accounts";
import type { SecretScope } from "@/lib/api/secrets";
import { useWhoCan } from "@/lib/api/who-can";
import { splitGiven } from "@/lib/accounts";
import { cn } from "@/lib/utils";

import { AccountRow } from "./account-row";

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
  appsHref = "?tab=apps",
}: {
  scope: SecretScope;
  orgId: string;
  canOwn: boolean;
  /** The agent whose Access tab this is: its accounts come first, the others are collapsed under "Not given to …". */
  agent?: { id: string; name: string };
  banner?: ReactNode;
  kind?: "app" | "website" | "ai_key" | null;
  /** Where **Connect an app or website** / **Give <Agent> an app** leads in this wave (the Apps tab). */
  appsHref?: string;
}) {
  const accounts = useAccounts(scope.kind === "workspace" ? { kind: "workspace", id: scope.id, agentId: agent?.id } : { kind: "organization", id: scope.id });
  const [addingKey, setAddingKey] = useState(false);
  const who = useWhoCan(
    scope.kind === "workspace" ? { workspaceId: scope.id } : { orgId: scope.id },
    scope.kind === "workspace" ? "workspace_owner" : "org_admin",
    "add_account",
  );

  if (accounts.isPending) return <Loading />;
  if (accounts.isError) return <ErrorState error={accounts.error} onRetry={() => accounts.refetch()} />;
  const items = accounts.data.items.filter((a) => !kind || a.kind === kind);
  const row = (a: Account) => (
    <AccountRow
      key={a.ref.id}
      account={a}
      scope={scope}
      orgId={orgId}
      workspaceId={scope.kind === "workspace" ? scope.id : undefined}
      agent={agent}
    />
  );

  const addEntries = agent ? (
    <>
      <Link className={cn(buttonVariants({ size: "sm" }))} href={appsHref}>
        Give {agent.name} an app
      </Link>
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
          <MenuItem asChild>
            <Link href={appsHref}>Connect an app or website</Link>
          </MenuItem>
        ) : null}
        <MenuItem onSelect={() => setAddingKey(true)}>Add an AI key</MenuItem>
      </MenuContent>
    </Menu>
  ) : null;

  const { given, notGiven } = agent ? splitGiven(items) : { given: items, notGiven: [] as Account[] };
  return (
    <div className="space-y-4">
      {banner}
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
                <Link className={cn(buttonVariants({ size: "sm" }))} href={appsHref}>
                  Give {agent.name} an app
                </Link>
              }
              ask={{ whoCan: who, canAct: canOwn }}
            />
          ) : (
            <EmptyState
              title="Nothing connected yet"
              when="Connect an app, a website or an AI key, and choose which agents can use it."
              action={
                <Link className={cn(buttonVariants({ size: "sm" }))} href={appsHref}>
                  Connect an app or website
                </Link>
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
    </div>
  );
}
