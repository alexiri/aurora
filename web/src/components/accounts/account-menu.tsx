"use client";

import { useQueryClient } from "@tanstack/react-query";
import { useState } from "react";

import { AccessDialog } from "@/components/credentials/give-access";
import { ErrorState } from "@/components/states";
import { Button } from "@/components/ui/button";
import { ConfirmDialog } from "@/components/ui/confirm-dialog";
import { Dialog, DialogClose, DialogContent } from "@/components/ui/dialog";
import { Menu, MenuContent, MenuItem, MenuTrigger } from "@/components/ui/menu";
import { accountKeys, type Account } from "@/lib/api/accounts";
import { useDeleteConnection, usePatchConnection } from "@/lib/api/apps";
import { ValueInput } from "@/components/credentials/value-input";
import { useDeleteSecret, usePatchSecret, useSetSecretValue, useVerifySecret, type SecretScope } from "@/lib/api/secrets";

/**
 * The row's menu, in the order of `14 §1`: Give access, Rename, Reconnect or Replace, Test, Turn off (or Turn on),
 * Remove. P5-UX-11 and P5-UX-14 edit this file for the organization's actions and the checks. Nothing here for
 * people who may not manage the account: the caller renders no menu then.
 */
export function AccountMenu({
  account,
  scope,
  orgId,
  contextAgentId,
  onReconnect,
}: {
  account: Account;
  /** The scope the account lives in (the list's scope for its own accounts; the organization for shared ones). */
  scope: SecretScope;
  orgId: string;
  contextAgentId?: string;
  onReconnect?: () => void;
}) {
  const qc = useQueryClient();
  const refresh = () => void qc.invalidateQueries({ queryKey: accountKeys.all });
  const isConn = account.ref.type === "connection";
  const [giving, setGiving] = useState(false);
  const [renaming, setRenaming] = useState(false);
  const [removing, setRemoving] = useState(false);
  const [replacing, setReplacing] = useState(false);
  const setValue = useSetSecretValue(scope, account.ref.id);
  const [newValue, setNewValue] = useState("");
  const [label, setLabel] = useState(account.name);
  const credScope = { kind: scope.kind, id: scope.id } as const;
  const patchConn = usePatchConnection(credScope);
  const delConn = useDeleteConnection(credScope);
  const patchSecret = usePatchSecret(scope, account.ref.id);
  const verify = useVerifySecret(scope, account.ref.id);
  const delSecret = useDeleteSecret(scope, account.ref.id);
  const off = account.health.state === "disabled";
  const error = patchConn.error ?? delConn.error ?? patchSecret.error ?? verify.error ?? delSecret.error;

  return (
    <>
      <Menu>
        <MenuTrigger asChild>
          <Button variant="outline" size="sm" aria-label={`More for ${account.name}`}>
            More
          </Button>
        </MenuTrigger>
        <MenuContent>
          {account.can.give_access ? <MenuItem onSelect={() => setGiving(true)}>Give access</MenuItem> : null}
          <MenuItem onSelect={() => setRenaming(true)}>Rename</MenuItem>
          {isConn && onReconnect ? <MenuItem onSelect={onReconnect}>Reconnect</MenuItem> : null}
          {!isConn ? <MenuItem onSelect={() => setReplacing(true)}>Replace</MenuItem> : null}
          {!isConn ? <MenuItem onSelect={() => verify.mutate(undefined, { onSuccess: refresh })}>Test</MenuItem> : null}
          {isConn ? (
            <MenuItem onSelect={() => patchConn.mutate({ id: account.ref.id, status: off ? "active" : "disabled" }, { onSuccess: refresh })}>
              {off ? "Turn on" : "Turn off"}
            </MenuItem>
          ) : null}
          <MenuItem onSelect={() => setRemoving(true)}>Remove</MenuItem>
        </MenuContent>
      </Menu>
      {error ? <ErrorState error={error} /> : null}
      {giving ? (
        <AccessDialog
          open
          onOpenChange={setGiving}
          title={account.name}
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
        description="Every agent loses access to it. This cannot be undone."
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
