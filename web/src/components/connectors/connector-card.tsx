"use client";

import { useState } from "react";

import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { useDeleteConnector, useRotateConnectorToken, type ConnectorAdmin } from "@/lib/api/connectors";

import { ApprovalPanel } from "./approval-panel";
import { ConnectorError, ConnectorIcon, ConnectorStatusChip, ManagedBadge } from "./shared";

/** One connector in the list: identity, status, counts, and either Manage (managed) or the self-hosted actions. */
export function ConnectorCard({
  c,
  onManage,
  onToken,
}: {
  c: ConnectorAdmin;
  onManage: (id: string) => void;
  onToken: (id: string, token: string) => void;
}) {
  const rotate = useRotateConnectorToken();
  const del = useDeleteConnector();
  const [confirmDel, setConfirmDel] = useState(false);
  const hosts = c.host_status.length;
  return (
    <li>
      <Card className="space-y-3">
        <div className="flex flex-wrap items-center gap-x-3 gap-y-2">
          <ConnectorIcon id={c.id} name={c.display_name} size={32} />
          <h3 className="font-display text-base font-semibold text-ink">{c.display_name}</h3>
          <code className="font-mono text-xs text-muted">{c.id}</code>
          <ManagedBadge managed={c.managed} />
          <ConnectorStatusChip c={c} />
          {c.version ? <span className="text-xs text-muted">v{c.version}</span> : null}
          <span className="text-xs text-muted sm:ml-auto">
            {c.integration_count} integration(s) ·{" "}
            {c.managed ? `${hosts} host(s) reporting` : `${c.replicas.length} replica(s)`}
          </span>
        </div>
        {c.managed ? (
          <Button variant="outline" size="sm" aria-label={`Manage ${c.display_name}`} onClick={() => onManage(c.id)}>
            Manage
          </Button>
        ) : (
          <>
            {c.status === "pending_approval" ? <ApprovalPanel c={c} /> : null}
            <div className="flex flex-wrap gap-2">
              <Button
                variant="outline"
                size="sm"
                disabled={rotate.isPending}
                onClick={() =>
                  rotate.mutate(c.id, {
                    onSuccess: (r) => {
                      onToken(c.id, r.token);
                      rotate.reset();
                    },
                  })
                }
              >
                Rotate token
              </Button>
              {confirmDel ? (
                <>
                  <Button variant="danger" size="sm" disabled={del.isPending} onClick={() => del.mutate(c.id)}>
                    Confirm delete
                  </Button>
                  <Button variant="outline" size="sm" onClick={() => setConfirmDel(false)}>
                    Cancel
                  </Button>
                </>
              ) : (
                <Button variant="danger" size="sm" onClick={() => setConfirmDel(true)}>
                  Delete
                </Button>
              )}
            </div>
            <ConnectorError error={rotate.error ?? del.error} />
          </>
        )}
      </Card>
    </li>
  );
}
