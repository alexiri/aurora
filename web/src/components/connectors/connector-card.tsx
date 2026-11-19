"use client";

import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { ConfirmButton } from "@/components/ui/confirm-button";
import { useConnectorTypes, useDeleteConnector, useRotateConnectorToken, type ConnectorAdmin } from "@/lib/api/connectors";
import { plural } from "@/lib/utils";

import { ApprovalPanel } from "./approval-panel";
import { ConnectorError, ConnectorIcon, ConnectorStatusChip, UnsignedBadge } from "./shared";

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
  const hosts = c.host_status.length;
  // A managed connector of a type the live hosts announced unsigned (dev only, D-142). The
  // catalogue is the source: the connector row carries no signature state of its own.
  const types = useConnectorTypes();
  const unsigned = c.managed && types.data?.items.find((t) => t.type_id === c.type_id)?.signed === false;
  return (
    <li>
      <Card className="space-y-3">
        <div className="flex flex-wrap items-center gap-x-3 gap-y-2">
          <ConnectorIcon id={c.id} name={c.display_name} hasIcon={c.has_icon} size={32} />
          <h3 className="font-display text-base font-semibold text-ink">{c.display_name}</h3>
          {/* (P5-POLISH-11, F2) One chip, the status; who runs it is plain text, and the raw id is not shown. */}
          <ConnectorStatusChip c={c} />
          {unsigned ? <UnsignedBadge /> : null}
          <span className="text-xs text-muted">{c.managed ? "Managed" : "Self-hosted"}</span>
          {c.version ? <span className="text-xs text-muted">v{c.version}</span> : null}
          <span className="text-xs text-muted sm:ml-auto">
            {plural(c.integration_count, "integration")} ·{" "}
            {c.managed ? `${plural(hosts, "host")} reporting` : plural(c.replicas.length, "replica")}
          </span>
        </div>
        {c.managed ? (
          <Button variant="outline" size="sm" aria-label={`Manage ${c.display_name}`} onClick={() => onManage(c.id)}>
            Manage
          </Button>
        ) : (
          <>
            {c.status === "pending_approval" || c.update_pending ? <ApprovalPanel c={c} /> : null}
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
              <ConfirmButton size="sm" pending={del.isPending} onConfirm={() => del.mutate(c.id)} />
            </div>
            <ConnectorError error={rotate.error ?? del.error} />
          </>
        )}
      </Card>
    </li>
  );
}
