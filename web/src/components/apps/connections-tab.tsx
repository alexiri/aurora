"use client";

import { useParams } from "next/navigation";
import { useCallback, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";

import { EmptyState, ErrorState, Loading } from "@/components/states";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Notice } from "@/components/ui/notice";
import { errorMessage } from "@/lib/api/errors";
import {
  appKeys,
  connectWindowPath,
  openConnectWindow,
  useConnectedListener,
  useConnections,
  useDeleteConnection,
  usePatchConnection,
  type Connection,
  type CredScope,
} from "@/lib/api/apps";

type ListScope = CredScope & { kind: "workspace" | "organization" };

const STATUS_LABEL: Record<string, string> = {
  active: "Active",
  reauth_required: "Reconnect required",
  disabled: "Disabled",
  orphaned: "Orphaned (package removed)",
};

function ConnectionRow({ scope, c, canOwn, org, ws }: { scope: ListScope; c: Connection; canOwn: boolean; org?: string; ws?: string }) {
  const patch = usePatchConnection(scope);
  const del = useDeleteConnection(scope);
  const [confirm, setConfirm] = useState(false);
  const bad = c.status === "reauth_required" || c.status === "orphaned";
  const lost = c.injecting.filter((i) => i.lost_to_app_id);
  const ok = c.injecting.filter((i) => !i.lost_to_app_id);
  return (
    <li className="space-y-2 rounded-card border border-line bg-surface p-4 shadow-card">
      <div className="flex flex-wrap items-center justify-between gap-x-3 gap-y-2">
        <div className="flex min-w-0 flex-wrap items-center gap-x-2 gap-y-1">
          <span className="font-medium text-ink">{c.label}</span>
          <span className="font-mono text-xs text-muted">{c.app_id}</span>
          <Badge tone={bad ? "danger" : c.status === "active" ? "accent" : "neutral"}>{STATUS_LABEL[c.status] ?? c.status}</Badge>
          {c.instance ? (
            <Badge className="font-mono normal-case tracking-normal" title={`Self-hosted instance: ${c.instance.label}`}>
              {c.instance.host}
            </Badge>
          ) : null}
        </div>
        {canOwn ? (
          <div className="flex flex-wrap gap-2">
            {org && ws && c.status !== "orphaned" ? (
              <Button size="sm" variant="outline" aria-label={`Reconnect ${c.label}`} onClick={() => openConnectWindow(connectWindowPath(org, ws, c.app_id, c.id))}>
                Reconnect
              </Button>
            ) : null}
            {(c.status === "active" || c.status === "disabled") && c.status_reason !== "instance_deleted" ? (
              <Button
                size="sm"
                variant="outline"
                disabled={patch.isPending}
                onClick={() => patch.mutate({ id: c.id, status: c.status === "active" ? "disabled" : "active" })}
              >
                {c.status === "active" ? "Disable" : "Enable"}
              </Button>
            ) : null}
            <Button size="sm" variant="danger" onClick={() => setConfirm(true)} aria-label={`Delete ${c.label}`}>Delete</Button>
          </div>
        ) : null}
      </div>
      {c.status_reason ? (
        <p className="text-sm text-ink-2">
          {c.status_reason === "instance_deleted"
            ? "The app instance of this connection was deleted. Connect again."
            : c.status === "reauth_required" && c.status_reason.startsWith("plugin_major_upgrade:")
              ? `${c.app_id} was upgraded to a version with different credential fields. Reconnect to keep using it.`
              : c.status_reason}
        </p>
      ) : null}
      <p className="text-xs text-muted">
        {c.scopes.length ? `Scopes: ${c.scopes.join(", ")} · ` : ""}
        {c.connected_by ? `Connected by ${c.connected_by.display_name} · ` : ""}
        {c.last_used_at ? `Last used ${new Date(c.last_used_at).toLocaleString()}` : "Never used"}
      </p>
      {c.missing_scopes.length ? (
        <p role="status" className="text-xs text-pending-ink">
          Some requested scopes were not granted: {c.missing_scopes.join(", ")}. Reconnect and approve them to restore full access.
        </p>
      ) : null}
      {c.database ? (
        <Notice tone="neutral" aria-label={`How to connect to ${c.label}`} className="space-y-1 text-xs">
          <p>
            Service name <code className="break-all font-mono text-ink">{c.database.service}</code>. In the agent&apos;s workspace run{" "}
            <code className="break-all font-mono text-ink">psql service={c.database.service} -c &apos;select 1&apos;</code>.
          </p>
          <p>
            The agent connects to the Aurora gateway with its gateway credential; the database password stays here. If you grant two
            database connections with the same name to one agent, each name gets <code className="font-mono">-</code> and the first 8
            characters of the connection id (<code className="font-mono">{c.id.replaceAll("-", "").slice(0, 8)}</code> here).
          </p>
        </Notice>
      ) : null}
      {ok.length ? <p className="text-xs text-ink-2">Injecting credentials for: {ok.map((i) => i.host).join(", ")}</p> : null}
      {lost.length ? (
        <p className="text-xs text-pending-ink">
          Not injecting for {lost.map((i) => `${i.host} (claimed by ${i.lost_to_app_id})`).join(", ")}
        </p>
      ) : null}
      {patch.error ? <Notice tone="danger" role="alert">{errorMessage(patch.error)}</Notice> : null}
      {confirm ? (
        <Notice tone="danger" role="alertdialog" aria-label={`Delete ${c.label}`} className="space-y-2">
          <p>Delete connection {c.label}? Agents will lose access through it.</p>
          {del.error ? <p role="alert">{errorMessage(del.error)}</p> : null}
          <div className="flex gap-2">
            <Button size="sm" variant="danger" disabled={del.isPending} onClick={() => del.mutate(c.id, { onSuccess: () => setConfirm(false) })}>Confirm delete</Button>
            <Button size="sm" variant="outline" onClick={() => setConfirm(false)}>Cancel</Button>
          </div>
        </Notice>
      ) : null}
    </li>
  );
}

export function ConnectionsTab({ scope, canOwn }: { scope: ListScope; canOwn: boolean }) {
  const params = useParams<{ org?: string; ws?: string }>();
  const list = useConnections(scope);
  const qc = useQueryClient();
  const refresh = useCallback(() => void qc.invalidateQueries({ queryKey: appKeys.connections(scope) }), [qc, scope]);
  useConnectedListener(refresh);
  if (list.isPending) return <Loading />;
  if (list.isError) return <ErrorState error={list.error} onRetry={() => list.refetch()} />;
  if (list.data.items.length === 0) return <EmptyState title="No connections yet">Connect an app from the Apps tab.</EmptyState>;
  return (
    <ul className="space-y-2" aria-label="Connections">
      {list.data.items.map((c) => (
        <ConnectionRow
          key={c.id}
          scope={scope}
          c={c}
          canOwn={canOwn}
          org={scope.kind === "workspace" ? params.org : undefined}
          ws={scope.kind === "workspace" ? params.ws : undefined}
        />
      ))}
    </ul>
  );
}
