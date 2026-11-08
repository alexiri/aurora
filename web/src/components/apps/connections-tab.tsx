"use client";

import { useParams } from "next/navigation";
import { useCallback, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";

import { EmptyState, ErrorState, Loading } from "@/components/states";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
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
    <li className="space-y-2 rounded-md border border-neutral-200 p-3">
      <div className="flex flex-wrap items-baseline justify-between gap-2">
        <div>
          <span className="font-medium">{c.label}</span>{" "}
          <span className="text-xs text-neutral-500">{c.app_id}</span>{" "}
          <span
            className={bad ? "rounded bg-red-100 px-1.5 py-0.5 text-xs text-red-800" : "rounded bg-neutral-100 px-1.5 py-0.5 text-xs"}
          >
            {STATUS_LABEL[c.status] ?? c.status}
          </span>
          {c.instance ? (
            <>
              {" "}
              <Badge className="font-mono normal-case tracking-normal" title={`Self-hosted instance: ${c.instance.label}`}>
                {c.instance.host}
              </Badge>
            </>
          ) : null}
        </div>
        {canOwn ? (
          <div className="flex gap-1">
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
            <Button size="sm" variant="outline" onClick={() => setConfirm(true)} aria-label={`Delete ${c.label}`}>Delete</Button>
          </div>
        ) : null}
      </div>
      {c.status_reason ? (
        <p className="text-sm text-neutral-600">
          {c.status_reason === "instance_deleted"
            ? "The app instance of this connection was deleted. Connect again."
            : c.status_reason}
        </p>
      ) : null}
      <p className="text-xs text-neutral-500">
        {c.scopes.length ? `Scopes: ${c.scopes.join(", ")} · ` : ""}
        {c.connected_by ? `Connected by ${c.connected_by.display_name} · ` : ""}
        {c.last_used_at ? `Last used ${new Date(c.last_used_at).toLocaleString()}` : "Never used"}
      </p>
      {c.missing_scopes.length ? (
        <p role="status" className="text-xs text-amber-700">
          Some requested scopes were not granted: {c.missing_scopes.join(", ")}. Reconnect and approve them to restore full access.
        </p>
      ) : null}
      {ok.length ? <p className="text-xs">Injecting credentials for: {ok.map((i) => i.host).join(", ")}</p> : null}
      {lost.length ? (
        <p className="text-xs text-amber-700">
          Not injecting for {lost.map((i) => `${i.host} (claimed by ${i.lost_to_app_id})`).join(", ")}
        </p>
      ) : null}
      {patch.error ? <p role="alert" className="text-sm text-red-700">{errorMessage(patch.error)}</p> : null}
      {confirm ? (
        <div role="alertdialog" aria-label={`Delete ${c.label}`} className="space-y-2 text-sm">
          <p>Delete connection {c.label}? Agents will lose access through it.</p>
          {del.error ? <p role="alert" className="text-red-700">{errorMessage(del.error)}</p> : null}
          <div className="flex gap-2">
            <Button size="sm" disabled={del.isPending} onClick={() => del.mutate(c.id, { onSuccess: () => setConfirm(false) })}>Confirm delete</Button>
            <Button size="sm" variant="outline" onClick={() => setConfirm(false)}>Cancel</Button>
          </div>
        </div>
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
