"use client";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { ItemList, ListRow } from "@/components/ui/section";
import { useConnectorTypes, type ConnectorAdmin } from "@/lib/api/connectors";
import { notAvailableYet } from "@/lib/chat-platforms";

import { plural } from "@/lib/utils";

import { ConnectorIcon, ConnectorNextStep, ConnectorStatusChip } from "./shared";

/**
 * (P5-REFINE-18 #7, admin-6) The platforms first: what Aurora can connect people through, with the ones it cannot yet.
 * A type the API lists is "Available" (Add, or Added once it is); the rest of the product's names are "Not available yet".
 */
export function PlatformsList({ connectors, onAdd, onManage }: { connectors: ConnectorAdmin[]; onAdd: () => void; onManage: (id: string) => void }) {
  const types = useConnectorTypes();
  const listed = types.data?.items ?? [];
  const soon = notAvailableYet(listed);
  return (
    <div className="space-y-3">
      <ItemList aria-label="Chat platforms">
        {listed.map((t) => {
          const connector = connectors.find((c) => c.type_id === t.type_id);
          return (
            <ListRow
              key={t.type_id}
              actions={
                t.added ? (
                  <>
                    <Badge tone="accent">Added</Badge>
                    {connector ? (
                      <Button variant="outline" size="sm" aria-label={`Manage ${connector.display_name}`} onClick={() => onManage(connector.id)}>
                        Manage
                      </Button>
                    ) : null}
                  </>
                ) : (
                  <Button variant="outline" size="sm" aria-label={`Add ${t.name}`} onClick={onAdd}>
                    Add
                  </Button>
                )
              }
            >
              <ConnectorIcon id={t.type_id} name={t.name} iconB64={t.icon_png_b64} hasIcon={false} />
              <div className="min-w-0 flex-1 basis-48">
                <p className="text-sm font-semibold text-ink">{t.name}</p>
                <p className="flex flex-wrap items-center gap-x-2 text-sm text-ink-2">
                  {connector ? <ConnectorStatusChip c={connector} /> : "Available"}
                  {connector && connector.host_running ? (
                    <span>
                      · {plural(connector.replicas.length, "replica")}
                      {connector.status === "active" ? " · Ready" : ""}
                    </span>
                  ) : null}
                </p>
                {connector ? <ConnectorNextStep c={connector} /> : null}
              </div>
            </ListRow>
          );
        })}
        {soon.map((p) => (
          <ListRow key={p.id} actions={<Badge>Not available yet</Badge>}>
            <ConnectorIcon id={p.id} name={p.name} hasIcon={false} />
            <div className="min-w-0 flex-1 basis-48">
              <p className="text-sm font-semibold text-ink">{p.name}</p>
              <p className="text-sm text-ink-2">Not available yet</p>
            </div>
          </ListRow>
        ))}
      </ItemList>
      {/* The choose step also holds the self-hosted path, and says so when no host is running. */}
      <Button variant="outline" onClick={onAdd}>
        Add chat platform
      </Button>
    </div>
  );
}
