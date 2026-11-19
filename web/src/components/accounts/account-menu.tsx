"use client";

import { useQueryClient } from "@tanstack/react-query";
import { useState } from "react";

import { ErrorState } from "@/components/states";
import { Button } from "@/components/ui/button";
import { ConfirmDialog } from "@/components/ui/confirm-dialog";
import { Dialog, DialogClose, DialogContent } from "@/components/ui/dialog";
import { Menu, MenuContent, MenuItem, MenuSeparator, MenuTrigger } from "@/components/ui/menu";
import { accountKeys, useRemoveAccess, type Account } from "@/lib/api/accounts";
import { useDeleteConnection, usePatchConnection } from "@/lib/api/apps";
import { ValueInput } from "@/components/credentials/value-input";

import { useOrg } from "@/lib/api/queries";

import { EditWebsite } from "./edit-website";
import { OrgGiveAccessDialog } from "./org-give-access";
import { useDeleteSecret, usePatchSecret, useSetSecretValue, type SecretScope } from "@/lib/api/secrets";

/**
 * The row's menu (P5-POLISH-6, C2): Test the connection, Sign in again (or Replace), Rename, then Remove from <Agent> and
 * Delete account… in danger ink. Outside an agent's list it also has Give access and Turn off (or Turn on). P5-UX-11 and P5-UX-14 edit this file for the organization's actions and the checks. Nothing here for
 * people who may not manage the account: the caller renders no menu then.
 */
export function AccountMenu({
  account,
  scope,
  orgId,
  contextAgentId,
  agent,
  onReconnect,
  onTest,
  replacing,
  onReplacingChange,
}: {
  /** The agent whose Access page this is: adds Remove from <Agent> and the Delete account… wording. */
  agent?: { id: string; name: string };
  account: Account;
  /** The scope the account lives in (the list's scope for its own accounts; the organization for shared ones). */
  scope: SecretScope;
  orgId: string;
  contextAgentId?: string;
  onReconnect?: () => void;
  /** Test: the row runs it and shows the result in place of its health line (C-943). */
  onTest?: () => void;
  /** The Replace dialog of a website or AI key, opened by the menu or by the health line's fix. */
  replacing: boolean;
  onReplacingChange: (open: boolean) => void;
}) {
  const qc = useQueryClient();
  const refresh = () => void qc.invalidateQueries({ queryKey: accountKeys.all });
  const isConn = account.ref.type === "connection";
  const org = useOrg(account.scope === "organization" ? orgId : undefined);
  const [giving, setGiving] = useState(false);
  const [renaming, setRenaming] = useState(false);
  const [removing, setRemoving] = useState(false);
  const setReplacing = onReplacingChange;
  const [editingSite, setEditingSite] = useState(false);
  const setValue = useSetSecretValue(scope, account.ref.id);
  const [newValue, setNewValue] = useState("");
  const [label, setLabel] = useState(account.name);
  const credScope = { kind: scope.kind, id: scope.id } as const;
  const patchConn = usePatchConnection(credScope);
  const delConn = useDeleteConnection(credScope);
  const patchSecret = usePatchSecret(scope, account.ref.id);
  const delSecret = useDeleteSecret(scope, account.ref.id);
  const removeFromAgent = useRemoveAccess();
  const mine = agent ? account.agents.find((a) => a.agent.id === agent.id) : undefined;
  const others = account.agents.filter((a) => a.agent.id !== agent?.id).map((a) => a.agent.name);
  const alsoRemovesFrom = others.length > 0 ? `Also removes it from ${others.join(", ")}` : "Nobody else uses it";
  const off = account.health.state === "disabled";
  const error = patchConn.error ?? delConn.error ?? patchSecret.error ?? delSecret.error;
  // A turned-off account runs nothing and an expired sign-in is fixed by Reconnect: there is nothing to test.
  const testable = !isConn || (account.health.state !== "disabled" && account.health.state !== "sign_in_expired");

  return (
    <>
      <Menu>
        <MenuTrigger asChild>
          <Button variant="outline" size="sm" aria-label={`More for ${account.name}`}>
            <span aria-hidden="true">…</span>
          </Button>
        </MenuTrigger>
        <MenuContent>
          {/* (P5-POLISH-6, C2) In an agent's list the agent already has access, so Give access and Turn off are not here:
              "Remove from <Agent>" covers turning it off for that agent. */}
          {!agent && account.can.give_access ? <MenuItem onSelect={() => setGiving(true)}>Give access</MenuItem> : null}
          {onTest && testable ? <MenuItem onSelect={onTest}>Test the connection</MenuItem> : null}
          {isConn && onReconnect ? <MenuItem onSelect={onReconnect}>Sign in again</MenuItem> : null}
          {account.kind === "website" ? <MenuItem onSelect={() => setEditingSite(true)}>Change website</MenuItem> : null}
          {!isConn ? <MenuItem onSelect={() => setReplacing(true)}>Replace</MenuItem> : null}
          <MenuItem onSelect={() => setRenaming(true)}>Rename</MenuItem>
          {/* (FIX-163) Its app instance was deleted: turning it on cannot work (the API refuses with `instance_deleted`). */}
          {!agent && isConn && !(off && account.status_reason === "instance_deleted") ? (
            <MenuItem onSelect={() => patchConn.mutate({ id: account.ref.id, status: off ? "active" : "disabled" }, { onSuccess: refresh })}>
              {off ? "Turn on" : "Turn off"}
            </MenuItem>
          ) : null}
          <MenuSeparator />
          {agent && mine ? (
            <MenuItem onSelect={() => removeFromAgent.mutate({ agentId: agent.id, account })} disabled={removeFromAgent.isPending}>
              <span>
                Remove from {agent.name}
                <span className="block text-xs text-muted">The account stays for other agents</span>
              </span>
            </MenuItem>
          ) : null}
          <MenuItem className="text-danger-ink" onSelect={() => setRemoving(true)}>
            <span>
              {agent ? "Delete account…" : "Remove"}
              {agent ? <span className="block text-xs text-muted">{alsoRemovesFrom}</span> : null}
            </span>
          </MenuItem>
        </MenuContent>
      </Menu>
      {error ? <ErrorState error={error} /> : null}
      {giving ? (
        <OrgGiveAccessDialog
          account={account}
          orgId={orgId}
          orgName={org.data?.name}
          useOrgPicker={scope.kind === "organization"}
          onOpenChange={setGiving}
          subject={{
            target: { kind: isConn ? "connection" : "secret", id: account.ref.id },
            scope,
            orgId,
            appId: account.app?.app_id,
            appWorkspaceId: scope.kind === "workspace" ? scope.id : undefined,
            customOnly: account.app?.custom_only,
            contextAgentId,
          }}
        />
      ) : null}
      <Dialog open={renaming} onOpenChange={setRenaming}>
        <DialogContent title={`Rename ${account.name}`}>
          <form
            className="space-y-3"
            onSubmit={(e) => {
              e.preventDefault();
              const name = label.trim();
              if (!name) return;
              const done = { onSuccess: () => { setRenaming(false); refresh(); } };
              if (isConn) patchConn.mutate({ id: account.ref.id, label: name }, done);
              else patchSecret.mutate({ name }, done);
            }}
          >
            <label className="block space-y-1 text-sm font-medium text-ink">
              Name
              <input className="block h-10 w-full rounded-control border border-line bg-surface px-3 text-sm font-normal" value={label} onChange={(e) => setLabel(e.target.value)} />
            </label>
            <div className="flex justify-end gap-2">
              <DialogClose asChild>
                <Button variant="outline" type="button">Cancel</Button>
              </DialogClose>
              <Button type="submit" disabled={!label.trim()}>Save</Button>
            </div>
          </form>
        </DialogContent>
      </Dialog>
      {account.kind === "website" ? (
        <EditWebsite scope={scope} accountId={account.ref.id} name={account.name} open={editingSite} onOpenChange={setEditingSite} />
      ) : null}
      <Dialog open={replacing} onOpenChange={(o) => { setReplacing(o); if (!o) setNewValue(""); }}>
        <DialogContent title={`Replace ${account.name}`}>
          <form
            className="space-y-3"
            aria-label={`Replace value of ${account.name}`}
            onSubmit={(e) => {
              e.preventDefault();
              if (!newValue) return;
              setValue.mutate({ value: newValue }, { onSuccess: () => { setReplacing(false); refresh(); }, onSettled: () => setNewValue("") });
            }}
          >
            <ValueInput id={`rv-${account.ref.id}`} label="New value" value={newValue} onChange={setNewValue} />
            {setValue.error ? <ErrorState error={setValue.error} /> : null}
            <div className="flex justify-end gap-2">
              <DialogClose asChild>
                <Button variant="outline" type="button">Cancel</Button>
              </DialogClose>
              <Button type="submit" disabled={!newValue || setValue.isPending}>Replace</Button>
            </div>
          </form>
        </DialogContent>
      </Dialog>
      <ConfirmDialog
        open={removing}
        onOpenChange={setRemoving}
        title={`Remove ${account.name}?`}
        description={agent ? `${alsoRemovesFrom}. This cannot be undone.` : "Every agent loses access to it. This cannot be undone."}
        confirmLabel="Remove"
        danger
        busy={delConn.isPending || delSecret.isPending}
        onConfirm={() => {
          const done = { onSuccess: () => { setRemoving(false); refresh(); } };
          if (isConn) delConn.mutate(account.ref.id, done);
          else delSecret.mutate(undefined, done);
        }}
      />
    </>
  );
}
