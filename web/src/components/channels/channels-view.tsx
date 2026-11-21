"use client";

import { useState } from "react";

import { fmtDate } from "@/components/account/key-table";
import { EmptyState, ErrorState, Loading } from "@/components/states";
import { Button } from "@/components/ui/button";
import { ConfirmAction } from "@/components/ui/confirm-dialog";
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
import {
  formFields,
  hasCapability,
  needsProbe,
  useConnectorAvailability,
  useConnectors,
  type ConnectorInfo,
} from "@/lib/api/connectors";
import { askersPhrase } from "@/components/overview/setup-request-row";
import { StatusLine } from "@/components/ui/status-line";
import { useOrgSetupRequests, type AppSetupRequest } from "@/lib/api/app-setup-requests";
import { useOrg, useSession } from "@/lib/api/queries";

import { ConnectorBlocked, NoConnectorsYet } from "./connector-blocked";
import { IntegrationDialog } from "./integration-dialog";
import { IntegrationStepper } from "./integration-stepper";
import { firstNameOf, personName } from "@/lib/workspace-name";
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
            <span className="text-sm font-medium text-ink">{personName(l.user.display_name)}</span>
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
  "This extends trust to the chat platform host: it can create links, and therefore speak and approve as any of your members whose verified email it asserts.";

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
      <p className="text-xs text-muted">Access details: stored</p>
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
          {disabled ? "Turn on" : "Turn off"}
        </Button>
        <Button variant="outline" size="sm" aria-expanded={links} onClick={() => setLinks((v) => !v)}>
          {links ? "Hide user links" : "User links"}
        </Button>
        <ConfirmAction
          label="Remove"
          size="sm"
          title="Remove this chat platform?"
          description="The agent stops answering in it and people lose their links to it. Messages already sent stay where they are."
          confirmLabel="Remove"
          pending={del.isPending}
          onConfirm={() => del.mutate(integration.id)}
        />
      </div>
      <MutationError error={patch.error ?? del.error} />
      {links ? <UserLinks integration={integration} /> : null}
    </li>
  );
}

/** "Carol Member", "Bob Builder and Carol Member", "Bob Builder, Carol Member and 2 others" (SEC-135: display names only). */
function askerNames(r: AppSetupRequest): string {
  const { who } = askersPhrase(r.askers.map((a) => personName(a.display_name)));
  return who;
}

/** "Carol gets" for one asker (by first name), "they get" for several. */
function gets(r: AppSetupRequest): string {
  return r.askers.length === 1 ? `${firstNameOf(r.askers[0]!.display_name) ?? "They"} gets` : "they get";
}

export function ChannelsView({ orgId }: { orgId: string }) {
  const org = useOrg(orgId);
  // (P5-REFINE-17 org-1, C-1047) The chat platform asks people made (owners and admins read them; the route 404s for others).
  const isOrgAdmin = org.data?.my_role === "owner" || org.data?.my_role === "admin";
  const requests = useOrgSetupRequests(orgId, isOrgAdmin);
  const asks = (requests.data ?? []).filter((r) => r.subject === "chat_platform");
  const integrations = useIntegrations(orgId);
  const connectors = useConnectors();
  const availability = useConnectorAvailability();
  const session = useSession();
  const [adding, setAdding] = useState<ConnectorInfo | null>(null);
  const [updating, setUpdating] = useState<ChannelIntegration | null>(null);

  if (integrations.isPending || connectors.isPending) return <Loading />;
  if (integrations.isError) return <ErrorState error={integrations.error} onRetry={() => integrations.refetch()} />;
  if (connectors.isError) return <ErrorState error={connectors.error} onRetry={() => connectors.refetch()} />;

  const byId = new Map(connectors.data.items.map((c) => [c.id, c]));
  // Types that exist but are not usable yet, each with who acts next (C-712). A failed
  // availability read only loses these notes; the usable connectors above still work.
  const blocked = (availability.data?.items ?? []).filter((a) => a.setup_state !== "active");
  const admins = availability.data?.instance_admins ?? [];
  const isInstanceAdmin = session.data?.user.is_instance_admin === true;
  const updatingConnector = updating ? (byId.get(updating.connector_id) ?? null) : null;

  return (
    <div className="space-y-8">
      <PageHeader
        title="Chat platforms"
        description={`Let people talk to agents from the chat tools ${org.data?.name ?? "your organization"} already uses.`}
        className="mb-6"
      />

      <Section id="connectors" title="Add a chat platform">
        {connectors.data.items.length > 0 ? (
          <ul className="space-y-3" aria-label="Chat platforms you can connect">
            {connectors.data.items.map((c) => (
              <li key={c.id} data-testid={`platform-${c.id}`} className="space-y-2">
                {/* (P5-REFINE-17 org-1) The same readiness as Administration: a running connector means Connect. */}
                <div className="flex flex-wrap items-center gap-x-3 gap-y-2 rounded-card border border-line bg-surface p-3 sm:px-4">
                  {/* eslint-disable-next-line @next/next/no-img-element */}
                  <img src={c.icon_url} alt="" width={36} height={36} className="size-9 shrink-0 rounded-control bg-subtle p-1.5" />
                  <span className="min-w-0 flex-1 basis-40">
                    <span className="block font-semibold text-ink">{c.display_name}</span>
                    <span className="block text-sm text-ink-2">Ready · connector running</span>
                  </span>
                  <StatusLine tone="ok">Ready</StatusLine>
                  <Button aria-label={`Connect ${c.display_name}`} onClick={() => setAdding(c)}>
                    Connect
                  </Button>
                </div>
                {asks
                  .filter((r) => r.connector_type === c.id)
                  .map((r) => (
                    <Notice key={r.id} tone="accent" role="status" data-testid="platform-ask" className="flex flex-wrap items-center gap-x-4 gap-y-2">
                      <span className="min-w-0 flex-1 basis-60">
                        {askerNames(r)} asked for {c.display_name}. Connect it and {gets(r)} a notice.
                      </span>
                      <Button size="sm" variant="outline" onClick={() => setAdding(c)}>
                        Connect {c.display_name}
                      </Button>
                    </Notice>
                  ))}
              </li>
            ))}
          </ul>
        ) : null}
        {asks
          .filter((r) => !r.connector_type)
          .map((r) => (
            <Notice key={r.id} tone="accent" role="status" data-testid="platform-ask">
              {askerNames(r)} asked for a chat platform. Connect one and {gets(r)} a notice.
            </Notice>
          ))}
        {blocked.length > 0 ? (
          <ul className="space-y-2" aria-label="Chat platforms that are not ready">
            {blocked.map((b) => (
              <ConnectorBlocked key={b.type_id} item={b} admins={admins} isInstanceAdmin={isInstanceAdmin} />
            ))}
          </ul>
        ) : null}
        {connectors.data.items.length === 0 && blocked.length === 0 ? (
          <NoConnectorsYet admins={admins} isInstanceAdmin={isInstanceAdmin} />
        ) : null}
      </Section>

      <Section id="integrations" title="Connected chat platforms">
        {integrations.data.items.length === 0 ? (
          <EmptyState title="No chat platforms connected yet">Pick a chat platform above to set one up.</EmptyState>
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

      {adding ? (
        needsProbe(formFields(adding.integration_setup)) ? (
          <IntegrationStepper key={adding.id} orgId={orgId} connector={adding} onClose={() => setAdding(null)} />
        ) : (
          <IntegrationDialog key={adding.id} orgId={orgId} connector={adding} onClose={() => setAdding(null)} />
        )
      ) : null}
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
