"use client";

import { useState } from "react";

import { ScopeGroups } from "@/components/grants/scope-groups";
import { Button } from "@/components/ui/button";
import { Notice } from "@/components/ui/notice";
import { errorMessage } from "@/lib/api/errors";
import { useApp, useConnections, usePatchConnection, type Connection, type CredScope } from "@/lib/api/apps";

/**
 * What the old Connections list showed per connection and an account row keeps under **Technical details**: why it
 * stopped, the scopes, the provider's token scopes, the database service line, the TLS warning and the hosts the
 * gateway injects for (D-241, FIX-136). Read from the connections list of the account's scope, which the page already
 * holds; nothing renders until it is open.
 */
export function ConnectionDetails({ scope, id, canManage }: { scope: Exclude<CredScope, { kind: "instance" }>; id: string; canManage: boolean }) {
  const list = useConnections(scope);
  const patch = usePatchConnection(scope);
  const c = list.data?.items.find((x) => x.id === id);
  if (!c) return null;
  const lost = c.injecting.filter((i) => i.lost_to_app_id);
  const ok = c.injecting.filter((i) => !i.lost_to_app_id);
  return (
    <div className="space-y-2">
      {c.status_reason ? (
        <p className="text-sm text-ink-2">
          {c.status_reason === "instance_deleted"
            ? "The self-hosted app of this connection was deleted. Connect again."
            : c.status === "reauth_required" && c.status_reason.startsWith("plugin_major_upgrade:")
              ? `${c.app_id} was upgraded to a version with different credential fields. Reconnect to keep using it.`
              : c.status_reason}
        </p>
      ) : null}
      <p className="text-xs text-muted">
        {c.scopes.length ? `Scopes: ${c.scopes.join(", ")} · ` : ""}
        {c.connected_by ? `Connected by ${c.connected_by.display_name} · ` : ""}
        {c.last_used_at ? `Last used ${new Date(c.last_used_at).toLocaleString()}` : "Never used"}
        {c.instance ? ` · Self-hosted: ${c.instance.base_url}` : ""}
      </p>
      {c.missing_scopes.length ? (
        <p role="status" className="text-xs text-pending-ink">
          Some requested scopes were not granted: {c.missing_scopes.join(", ")}. Reconnect and approve them to restore full access.
        </p>
      ) : null}
      {c.metadata.token_scopes ? (
        <p className="text-xs text-ink-2">
          Token scopes (reported by the provider): <span className="break-words font-mono text-ink">{c.metadata.token_scopes}</span>
          {c.access !== "read" && /(^|[ ,])repo($|[ ,])/.test(c.metadata.token_scopes)
            ? ". The repo scope allows writes: choose Read only for the agents to block them."
            : null}
        </p>
      ) : null}
      {canManage && c.connect_method ? <TokenScopes c={c} ws={c.workspace_id ?? (scope.kind === "workspace" ? scope.id : undefined)} patch={patch} /> : null}
      {c.database ? (
        <Notice tone="neutral" aria-label={`How to connect to ${c.label}`} className="space-y-1 text-xs">
          <p>
            Service name <code className="break-all font-mono text-ink">{c.database.service}</code>. In the agent&apos;s workspace run{" "}
            <code className="break-all font-mono text-ink">psql service={c.database.service} -c &apos;select 1&apos;</code>.
          </p>
          <p>
            The agent connects to the Aurora gateway with its gateway credential; the database password stays here. If you give two
            database accounts with the same name to one agent, each name gets <code className="font-mono">-</code> and the first 8
            characters of the account id (<code className="font-mono">{c.id.replaceAll("-", "").slice(0, 8)}</code> here).
          </p>
        </Notice>
      ) : null}
      {c.database && c.metadata.tls_verification === "off" ? (
        <Notice tone="pending" role="status" aria-label={`TLS verification is off for ${c.label}`} className="text-xs">
          The gateway does not verify this database server&apos;s TLS certificate: the database password goes to whichever server
          answers. Store the CA certificate (or use a publicly trusted one) and reconnect to turn verification back on.
        </Notice>
      ) : null}
      {ok.length ? <p className="text-xs text-ink-2">Injecting credentials for: {ok.map((i) => i.host).join(", ")}</p> : null}
      {lost.length ? (
        <p className="text-xs text-pending-ink">Not injecting for {lost.map((i) => `${i.host} (claimed by ${i.lost_to_app_id})`).join(", ")}</p>
      ) : null}
      {patch.error ? (
        <Notice tone="danger" role="alert">
          {errorMessage(patch.error)}
        </Notice>
      ) : null}
    </div>
  );
}

/**
 * (FIX-136, D-241) The scopes of a pasted token, "as you declared": shown for a connection made with a `fields` method that
 * declares scopes, editable by whoever can manage it. Aurora uses them only to hide and refuse tools the token cannot serve.
 */
function TokenScopes({ c, ws, patch }: { c: Connection; ws?: string; patch: ReturnType<typeof usePatchConnection> }) {
  const app = useApp(c.app_id, ws);
  const method = app.data?.connect?.find?.((m) => m.id === c.connect_method);
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState<string[]>(c.scopes);
  if (!method || method.kind !== "fields" || !method.scopes || method.scopes.length === 0) return null;
  const names = c.scopes.map((s) => method.scopes.find((x) => x.scope === s)?.name ?? s);
  return (
    <div className="space-y-1.5 text-xs" aria-label={`Token scopes of ${c.label}`}>
      <p className="text-ink-2">
        Token scopes (as you declared):{" "}
        <span className="text-ink">{names.length ? names.join(", ") : "none declared, so no tool is hidden"}</span>{" "}
        <Button
          variant="ghost"
          size="sm"
          onClick={() => {
            setDraft(c.scopes);
            setEditing((e) => !e);
          }}
        >
          {editing ? "Cancel" : "Edit scopes"}
        </Button>
      </p>
      {editing ? (
        <div className="space-y-2">
          <ScopeGroups scopes={method.scopes} selected={draft} onChange={setDraft} disabled={patch.isPending} />
          <Button size="sm" disabled={patch.isPending} onClick={() => patch.mutate({ id: c.id, scopes: draft }, { onSuccess: () => setEditing(false) })}>
            Save scopes
          </Button>
        </div>
      ) : null}
    </div>
  );
}
