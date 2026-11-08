"use client";

import { useState } from "react";

import { EmptyState } from "@/components/states";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { ConfirmDialog } from "@/components/ui/confirm-dialog";
import { DataTable, Td, Th } from "@/components/ui/data-table";
import { Notice } from "@/components/ui/notice";
import type { ApiKey } from "@/lib/api/account";
import { errorMessage } from "@/lib/api/errors";

export const fmtDate = (s?: string | null) => (s ? new Date(s).toLocaleString() : "—");

function keyStatus(k: ApiKey): string {
  if (k.revoked_at) return "Revoked";
  if (k.expires_at && new Date(k.expires_at) < new Date()) return "Expired";
  return "Active";
}

const statusTone = { Active: "accent", Revoked: "danger", Expired: "pending" } as const;

/** List of API keys with a confirmed revoke action. */
export function KeyTable({
  keys,
  canRevoke = true,
  onRevoke,
  revoking,
  error,
  showWorkspace,
}: {
  keys: ApiKey[];
  canRevoke?: boolean;
  onRevoke: (id: string) => Promise<unknown>;
  revoking?: boolean;
  error?: unknown;
  showWorkspace?: boolean;
}) {
  const [target, setTarget] = useState<ApiKey | null>(null);
  if (keys.length === 0) return <EmptyState title="No API keys" />;
  return (
    <div className="space-y-3">
      {error ? (
        <Notice tone="danger" role="alert">
          {errorMessage(error)}
        </Notice>
      ) : null}
      <DataTable caption="API keys">
        <thead>
          <tr>
            <Th>Name</Th>
            <Th>Key</Th>
            {showWorkspace ? <Th>Scope</Th> : null}
            <Th>Created</Th>
            <Th>Last used</Th>
            <Th>Expires</Th>
            <Th>Status</Th>
            <Th>
              <span className="sr-only">Actions</span>
            </Th>
          </tr>
        </thead>
        <tbody>
          {keys.map((k) => {
            const status = keyStatus(k);
            return (
              <tr key={k.id}>
                <Td className="font-medium">{k.name}</Td>
                <Td className="font-mono text-xs text-ink-2">{k.display_prefix}…</Td>
                {showWorkspace ? <Td>{k.workspace_id ? "Workspace" : "Organization"}</Td> : null}
                <Td className="whitespace-nowrap text-ink-2">{fmtDate(k.created_at)}</Td>
                <Td className="whitespace-nowrap text-ink-2">{fmtDate(k.last_used_at)}</Td>
                <Td className="whitespace-nowrap text-ink-2">{fmtDate(k.expires_at)}</Td>
                <Td>
                  <Badge tone={statusTone[status as keyof typeof statusTone]}>{status}</Badge>
                </Td>
                <Td className="text-right">
                  {canRevoke && !k.revoked_at ? (
                    <Button variant="outline" size="sm" aria-label={`Revoke ${k.name}`} onClick={() => setTarget(k)}>
                      Revoke
                    </Button>
                  ) : null}
                </Td>
              </tr>
            );
          })}
        </tbody>
      </DataTable>
      <ConfirmDialog
        open={target !== null}
        onOpenChange={(o) => !o && setTarget(null)}
        title="Revoke API key?"
        description={`Anything using "${target?.name ?? ""}" will stop working immediately.`}
        confirmLabel="Revoke key"
        danger
        busy={revoking}
        onConfirm={() => {
          if (!target) return;
          void onRevoke(target.id).then(
            () => setTarget(null),
            () => setTarget(null),
          );
        }}
      />
    </div>
  );
}
