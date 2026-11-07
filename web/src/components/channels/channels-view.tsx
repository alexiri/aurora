"use client";

import { useState } from "react";

import { fmtDate } from "@/components/account/key-table";
import { EmptyState, ErrorState, Loading } from "@/components/states";
import { Button } from "@/components/ui/button";
import {
  useDeleteIntegration,
  useDeleteUserLink,
  useIntegrations,
  useIntegrationUserLinks,
  usePatchIntegration,
  type ChannelIntegration,
} from "@/lib/api/channels";
import { hasCapability, useConnectors, type ConnectorInfo } from "@/lib/api/connectors";

import { IntegrationDialog } from "./integration-dialog";
import { MutationError, StatusChip } from "./shared";

function UserLinks({ integration }: { integration: ChannelIntegration }) {
  const q = useIntegrationUserLinks(integration.id, true);
  const del = useDeleteUserLink(integration.id);
  if (q.isPending) return <Loading />;
  if (q.isError) return <ErrorState error={q.error} onRetry={() => q.refetch()} />;
  if (q.data.items.length === 0) return <p className="text-sm text-neutral-500">No linked users yet.</p>;
  return (
    <div className="space-y-2">
      <ul aria-label={`User links for ${integration.tenant_label}`} className="divide-y divide-neutral-200 text-sm">
        {q.data.items.map((l) => (
          <li key={l.id} className="flex items-center gap-3 py-1.5">
            <span className="font-medium">{l.user.display_name}</span>
            <code className="text-xs">{l.platform_user_ref}</code>
            <span className="text-xs text-neutral-500">{l.method.replace(/_/g, " ")}</span>
            <Button
              variant="outline"
              size="sm"
              className="ml-auto"
              disabled={del.isPending}
              onClick={() => del.mutate(l.id)}
            >
              Unlink
            </Button>
          </li>
        ))}
      </ul>
      <MutationError error={del.error} />
    </div>
  );
}

const AUTOLINK_WARNING =
  "This extends trust to the connector: it can create links, and therefore speak and approve as any of your members whose verified email it asserts.";

/** RFE-001: org-admin opt-in for verified-email auto-linking (default off), with a confirm step. */
function AutolinkSwitch({
  integration,
  onChange,
  pending,
}: {
  integration: ChannelIntegration;
  onChange: (on: boolean) => void;
  pending: boolean;
}) {
  const on = (integration.config as Record<string, unknown>).verified_email_autolink === true;
  const [asking, setAsking] = useState<boolean | null>(null);
  return (
    <div className="space-y-1">
      <label className="flex items-center gap-2 text-sm">
        <input
          type="checkbox"
          checked={on}
          disabled={pending || asking !== null}
          onChange={(e) => setAsking(e.target.checked)}
        />
        Link members automatically by verified email
      </label>
      {asking !== null ? (
        <div role="alertdialog" aria-label="Confirm automatic linking" className="space-y-2 rounded-md border border-amber-300 p-2">
          <p className="text-sm text-amber-900">
            {asking ? AUTOLINK_WARNING : "Members will be asked to sign in to link their accounts. Existing links stay."}
          </p>
          <div className="flex gap-2">
            <Button
              size="sm"
              disabled={pending}
              onClick={() => {
                onChange(asking);
                setAsking(null);
              }}
            >
              {asking ? "Confirm enable" : "Confirm disable"}
            </Button>
            <Button variant="outline" size="sm" onClick={() => setAsking(null)}>
              Cancel
            </Button>
          </div>
        </div>
      ) : null}
    </div>
  );
}

function IntegrationRow({
  orgId,
  integration,
  connector,
  onUpdate,
}: {
  orgId: string;
  integration: ChannelIntegration;
  connector: ConnectorInfo | undefined;
  onUpdate: () => void;
}) {
  const patch = usePatchIntegration(orgId);
  const del = useDeleteIntegration(orgId);
  const [links, setLinks] = useState(false);
  const [confirmDel, setConfirmDel] = useState(false);
  const disabled = integration.status === "disabled";
  return (
    <li className="space-y-2 rounded-md border border-neutral-200 p-3">
      <div className="flex flex-wrap items-center gap-2">
        <h3 className="font-medium">{integration.tenant_label}</h3>
        <span className="text-sm text-neutral-600">{connector?.display_name ?? integration.connector_id}</span>
        <StatusChip status={integration.status} />
        <span className="ml-auto text-xs text-neutral-500">Added {fmtDate(integration.created_at)}</span>
      </div>
      {integration.status_reason ? <p className="text-sm text-amber-800">{integration.status_reason}</p> : null}
      <p className="text-sm text-neutral-700">
        Shared bot:{" "}
        {integration.shared_bot ? (
          <span>
            {integration.shared_bot.display_name} <code className="text-xs">{integration.shared_bot.handle}</code>
          </span>
        ) : (
          "none"
        )}
      </p>
      <p className="text-xs text-neutral-500">Credentials: stored</p>
      {connector && hasCapability(connector, "email_verified") ? (
        <AutolinkSwitch
          integration={integration}
          pending={patch.isPending}
          onChange={(on) => {
            const { _capabilities: _drop, ...rest } = integration.config as Record<string, unknown>;
            void _drop;
            patch.mutate({
              id: integration.id,
              body: { config: { ...rest, verified_email_autolink: on } as never },
            });
          }}
        />
      ) : null}
      <div className="flex flex-wrap gap-2">
        {connector ? (
          <Button variant="outline" size="sm" onClick={onUpdate}>
            Update credentials
          </Button>
        ) : null}
        <Button
          variant="outline"
          size="sm"
          disabled={patch.isPending}
          onClick={() => patch.mutate({ id: integration.id, body: { status: disabled ? "active" : "disabled" } })}
        >
          {disabled ? "Enable" : "Disable"}
        </Button>
        <Button variant="outline" size="sm" aria-expanded={links} onClick={() => setLinks((v) => !v)}>
          {links ? "Hide user links" : "User links"}
        </Button>
        {confirmDel ? (
          <>
            <Button size="sm" disabled={del.isPending} onClick={() => del.mutate(integration.id)}>
              Confirm remove
            </Button>
            <Button variant="outline" size="sm" onClick={() => setConfirmDel(false)}>
              Cancel
            </Button>
          </>
        ) : (
          <Button variant="outline" size="sm" onClick={() => setConfirmDel(true)}>
            Remove
          </Button>
        )}
      </div>
      <MutationError error={patch.error ?? del.error} />
      {links ? <UserLinks integration={integration} /> : null}
    </li>
  );
}

export function ChannelsView({ orgId }: { orgId: string }) {
  const integrations = useIntegrations(orgId);
  const connectors = useConnectors();
  const [adding, setAdding] = useState<ConnectorInfo | null>(null);
  const [updating, setUpdating] = useState<ChannelIntegration | null>(null);

  if (integrations.isPending || connectors.isPending) return <Loading />;
  if (integrations.isError) return <ErrorState error={integrations.error} onRetry={() => integrations.refetch()} />;
  if (connectors.isError) return <ErrorState error={connectors.error} onRetry={() => connectors.refetch()} />;

  const byId = new Map(connectors.data.items.map((c) => [c.id, c]));
  const updatingConnector = updating ? (byId.get(updating.connector_id) ?? null) : null;

  return (
    <div className="max-w-3xl space-y-6">
      <div>
        <h1 className="text-xl font-semibold">Channels</h1>
        <p className="text-sm text-neutral-600">
          Connect chat platforms so people can talk to agents where they already work.
        </p>
      </div>

      <section aria-label="Available connectors" className="space-y-2">
        <h2 className="font-medium">Add an integration</h2>
        {connectors.data.items.length === 0 ? (
          <p className="text-sm text-neutral-500">No connectors are available. Ask an instance administrator.</p>
        ) : (
          <ul className="flex flex-wrap gap-2">
            {connectors.data.items.map((c) => (
              <li key={c.id}>
                <Button variant="outline" onClick={() => setAdding(c)}>
                  {/* eslint-disable-next-line @next/next/no-img-element */}
                  <img src={c.icon_url} alt="" width={16} height={16} />
                  Connect {c.display_name}
                </Button>
              </li>
            ))}
          </ul>
        )}
      </section>

      <section aria-label="Integrations" className="space-y-2">
        <h2 className="font-medium">Integrations</h2>
        {integrations.data.items.length === 0 ? (
          <EmptyState title="No integrations yet">Pick a connector above to set one up.</EmptyState>
        ) : (
          <ul className="space-y-3">
            {integrations.data.items.map((i) => (
              <IntegrationRow
                key={i.id}
                orgId={orgId}
                integration={i}
                connector={byId.get(i.connector_id)}
                onUpdate={() => setUpdating(i)}
              />
            ))}
          </ul>
        )}
      </section>

      {adding ? <IntegrationDialog key={adding.id} orgId={orgId} connector={adding} onClose={() => setAdding(null)} /> : null}
      {updating ? (
        <IntegrationDialog
          key={updating.id}
          orgId={orgId}
          connector={updatingConnector}
          existing={updating}
          onClose={() => setUpdating(null)}
        />
      ) : null}
    </div>
  );
}
