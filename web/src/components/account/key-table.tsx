"use client";

import { useState } from "react";

import { EmptyState } from "@/components/states";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent } from "@/components/ui/dialog";
import type { ApiKey } from "@/lib/api/account";
import { errorMessage } from "@/lib/api/errors";

export const fmtDate = (s?: string | null) => (s ? new Date(s).toLocaleString() : "—");

function keyStatus(k: ApiKey): string {
  if (k.revoked_at) return "Revoked";
  if (k.expires_at && new Date(k.expires_at) < new Date()) return "Expired";
  return "Active";
}

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
    <div className="space-y-2">
      {error ? (
        <p role="alert" className="text-sm text-red-700">
          {errorMessage(error)}
        </p>
      ) : null}
      <div className="overflow-x-auto">
        <table className="w-full text-left text-sm">
          <thead className="text-neutral-500">
            <tr>
              <th className="py-2 pr-4 font-medium">Name</th>
              <th className="py-2 pr-4 font-medium">Key</th>
              {showWorkspace ? <th className="py-2 pr-4 font-medium">Scope</th> : null}
              <th className="py-2 pr-4 font-medium">Created</th>
              <th className="py-2 pr-4 font-medium">Last used</th>
              <th className="py-2 pr-4 font-medium">Expires</th>
              <th className="py-2 pr-4 font-medium">Status</th>
              <th className="py-2 font-medium">
                <span className="sr-only">Actions</span>
              </th>
            </tr>
          </thead>
          <tbody>
            {keys.map((k) => (
              <tr key={k.id} className="border-t border-neutral-200">
                <td className="py-2 pr-4">{k.name}</td>
                <td className="py-2 pr-4 font-mono">{k.display_prefix}…</td>
                {showWorkspace ? (
                  <td className="py-2 pr-4">{k.workspace_id ? "Workspace" : "Organization"}</td>
                ) : null}
                <td className="py-2 pr-4">{fmtDate(k.created_at)}</td>
                <td className="py-2 pr-4">{fmtDate(k.last_used_at)}</td>
                <td className="py-2 pr-4">{fmtDate(k.expires_at)}</td>
                <td className="py-2 pr-4">{keyStatus(k)}</td>
                <td className="py-2">
                  {canRevoke && !k.revoked_at ? (
                    <Button
                      variant="outline"
                      size="sm"
                      aria-label={`Revoke ${k.name}`}
                      onClick={() => setTarget(k)}
                    >
                      Revoke
                    </Button>
                  ) : null}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
      <Dialog open={target !== null} onOpenChange={(o) => !o && setTarget(null)}>
        <DialogContent
          title="Revoke API key?"
          description={`Anything using "${target?.name ?? ""}" will stop working immediately.`}
        >
          <div className="flex justify-end gap-2">
            <Button variant="outline" onClick={() => setTarget(null)}>
              Cancel
            </Button>
            <Button
              disabled={revoking}
              onClick={() => {
                if (!target) return;
                void onRevoke(target.id).then(
                  () => setTarget(null),
                  () => setTarget(null),
                );
              }}
            >
              Revoke key
            </Button>
          </div>
        </DialogContent>
      </Dialog>
    </div>
  );
}
