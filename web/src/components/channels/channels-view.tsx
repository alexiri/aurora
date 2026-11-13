"use client";

import { useState } from "react";

import { fmtDate } from "@/components/account/key-table";
import { EmptyState, ErrorState, Loading } from "@/components/states";
import { Button } from "@/components/ui/button";
import { CheckboxField } from "@/components/ui/form";
import { Notice } from "@/components/ui/notice";
import { PageHeader } from "@/components/ui/page-header";
import { ItemList, ListRow, Section } from "@/components/ui/section";
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
  if (q.data.items.length === 0) return <p className="text-sm text-muted">No linked users yet.</p>;
  return (
    <div className="space-y-2">
      <ItemList aria-label={`User links for ${integration.tenant_label}`} className="shadow-none">
        {q.data.items.map((l) => (
          <ListRow
            key={l.id}
            actions={
              <Button variant="outline" size="sm" disabled={del.isPending} onClick={() => del.mutate(l.id)}>
                Unlink
              </Button>
            }
          >
            <span className="text-sm font-medium text-ink">{l.user.display_name}</span>
            <code className="font-mono text-xs text-ink-2">{l.platform_user_ref}</code>
            <span className="text-xs text-muted">{l.method.replace(/_/g, " ")}</span>
          </ListRow>
        ))}
      </ItemList>
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
    <div className="space-y-2">
      <CheckboxField
        id={`autolink-${integration.id}`}
        label="Link members automatically by verified email"
        checked={on}
        disabled={pending || asking !== null}
        onChange={(v) => setAsking(v)}
      />
      {asking !== null ? (
        <Notice tone="pending" role="alertdialog" aria-label="Confirm automatic linking" className="space-y-2">
          <p>
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
        </Notice>
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
    <li className="space-y-3 rounded-card border border-line bg-surface p-4 shadow-card sm:p-5">
      <div className="flex flex-wrap items-center gap-x-3 gap-y-1">
        <h3 className="font-display text-base font-semibold text-ink">{integration.tenant_label}</h3>
        <span className="text-sm text-ink-2">{connector?.display_name ?? integration.connector_id}</span>
        <StatusChip status={integration.status} />
        <span className="text-xs text-muted sm:ml-auto">Added {fmtDate(integration.created_at)}</span>
      </div>
      {integration.status_reason ? <Notice tone="pending">{integration.status_reason}</Notice> : null}
      <p className="text-sm text-ink-2">
        Shared bot:{" "}
        {integration.shared_bot ? (
          <span>
            {integration.shared_bot.display_name} <code className="font-mono text-xs">{integration.shared_bot.handle}</code>
          </span>
        ) : (
          "none"
        )}
      </p>
      <p className="text-xs text-muted">Credentials: stored</p>
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
            <Button variant="danger" size="sm" disabled={del.isPending} onClick={() => del.mutate(integration.id)}>
              Confirm remove
            </Button>
            <Button variant="outline" size="sm" onClick={() => setConfirmDel(false)}>
              Cancel
            </Button>
          </>
        ) : (
          <Button variant="danger" size="sm" onClick={() => setConfirmDel(true)}>
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
    <div className="max-w-page space-y-8">
      <PageHeader
        title="Channels"
        description="Connect chat platforms so people can talk to agents where they already work."
        className="mb-6"
      />

      <Section id="connectors" title="Add an integration">
        {connectors.data.items.length === 0 ? (
          <p className="text-sm text-muted">No connectors are available. Ask an instance administrator.</p>
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
      </Section>

      <Section id="integrations" title="Integrations">
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
      </Section>

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
