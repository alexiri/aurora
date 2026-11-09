"use client";

import { useMemo, useState } from "react";

import { EmptyState, ErrorState, Loading } from "@/components/states";
import { AppIcon } from "@/components/ui/app-icon";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { ConfirmDialog } from "@/components/ui/confirm-dialog";
import { Notice } from "@/components/ui/notice";
import { ItemList, ListRow, Section } from "@/components/ui/section";
import { errorMessage } from "@/lib/api/errors";
import { appRestrictions, useApps, useConnections, type Connection } from "@/lib/api/apps";
import { useRemoveAgentConnectionGrant } from "@/lib/api/credential-grants";
import { useEffective, useGrants } from "@/lib/api/policy";
import { grantState } from "@/lib/grant-editor";

import { GrantEditor } from "./grant-editor";

const description = "What this agent may do through each connection: no access, full access, or a custom choice per tool.";

/** Owners: every connection in the agent's scope with its grant, an editor and removal (`GET/PUT/DELETE …/grants`). */
export function ConnectionGrants({ agentId, wsId, orgId }: { agentId: string; wsId: string; orgId: string }) {
  const wsConns = useConnections({ kind: "workspace", id: wsId });
  const orgConns = useConnections({ kind: "organization", id: orgId });
  const grants = useGrants(agentId);
  const apps = useApps(wsId);
  const remove = useRemoveAgentConnectionGrant(agentId);
  const [editing, setEditing] = useState<Connection | null>(null);
  const [removing, setRemoving] = useState<Connection | null>(null);

  const byConnection = useMemo(() => new Map((grants.data?.connections ?? []).map((g) => [g.connection_id, g])), [grants.data]);
  const connections = useMemo(
    () => [...(wsConns.data?.items ?? []), ...(orgConns.data?.items ?? [])].filter((c) => c.status !== "orphaned"),
    [wsConns.data, orgConns.data],
  );
  const appNames = useMemo(() => new Map((apps.data?.items ?? []).map((a) => [a.app_id, a])), [apps.data]);

  const section = (children: React.ReactNode) => (
    <Section id="connection-grants" title="Connections" description={description}>
      {children}
    </Section>
  );
  if (wsConns.isPending || grants.isPending) return section(<Loading />);
  if (wsConns.isError) return section(<ErrorState error={wsConns.error} onRetry={() => wsConns.refetch()} />);
  if (grants.isError) return section(<ErrorState error={grants.error} onRetry={() => grants.refetch()} />);

  return section(
    <>
      {orgConns.isError ? <ErrorState error={orgConns.error} onRetry={() => orgConns.refetch()} /> : null}
      {remove.isError ? (
        <Notice tone="danger" role="alert">
          Could not remove access: {errorMessage(remove.error)}
        </Notice>
      ) : null}
      {connections.length === 0 ? (
        <EmptyState title="No connections in scope">Connect an app on the Credentials page, then give this agent access here.</EmptyState>
      ) : (
        <ItemList aria-label="Connection grants">
          {connections.map((c) => {
            const grant = byConnection.get(c.id) ?? null;
            const state = grantState(grant);
            const app = appNames.get(c.app_id);
            const restricted = appRestrictions(grant, app);
            return (
              <ListRow
                key={c.id}
                actions={
                  <>
                    <Button size="sm" variant="outline" aria-label={`${grant ? "Edit" : "Give"} access to ${c.label}`} onClick={() => setEditing(c)}>
                      {grant ? "Edit" : "Give access"}
                    </Button>
                    {grant ? (
                      <Button size="sm" variant="danger" aria-label={`Remove access to ${c.label}`} onClick={() => setRemoving(c)}>
                        Remove
                      </Button>
                    ) : null}
                  </>
                }
              >
                <AppIcon appId={c.app_id} name={app?.name ?? c.app_id} hasDarkIcon={app?.has_dark_icon} size={32} />
                <div className="min-w-0 flex-1 space-y-0.5 text-sm">
                  <div className="flex flex-wrap items-center gap-x-2 gap-y-1">
                    <span className="break-words font-medium text-ink">{c.label}</span>
                    <span className="text-xs text-muted">
                      {app?.name ?? c.app_id} · {c.scope}
                    </span>
                    {restricted.custom_only ? <Badge>Custom access only</Badge> : null}
                  </div>
                  <p className="text-xs text-ink-2">
                    <Badge tone={state.kind === "none" ? "neutral" : "accent"}>{state.kind === "none" ? "None" : state.kind === "full" ? "Full" : "Custom"}</Badge>{" "}
                    <span data-testid="grant-state">{state.label}</span>
                  </p>
                </div>
              </ListRow>
            );
          })}
        </ItemList>
      )}
      {editing ? (
        <GrantEditor
          key={editing.id}
          agentId={agentId}
          wsId={wsId}
          connection={editing}
          grant={byConnection.get(editing.id) ?? null}
          restrictions={byConnection.get(editing.id)}
          open
          onOpenChange={(o) => {
            if (!o) setEditing(null);
          }}
        />
      ) : null}
      <ConfirmDialog
        open={removing !== null}
        onOpenChange={(o) => {
          if (!o) setRemoving(null);
        }}
        title={`Remove access to ${removing?.label ?? ""}`}
        description="The agent can no longer use this connection. Its custom tool choices are discarded."
        confirmLabel="Remove access"
        danger
        busy={remove.isPending}
        onConfirm={() => {
          if (removing) remove.mutate(removing.id, { onSettled: () => setRemoving(null) });
        }}
      />
    </>,
  );
}

/**
 * Everyone else: the decisions the gateway would apply, from the effective view (which they may read). Grants
 * themselves are owner-only, so no editing and no per-connection grant mode here.
 */
export function ConnectionGrantsReadOnly({ agentId, wsId }: { agentId: string; wsId: string }) {
  const effective = useEffective(agentId);
  const apps = useApps(wsId);
  const names = useMemo(() => new Map((apps.data?.items ?? []).map((a) => [a.app_id, a.name])), [apps.data]);

  const rows = useMemo(() => {
    const m = new Map<string, { appId: string; allow: number; ask: number; block: number }>();
    for (const t of effective.data?.tools ?? []) {
      const r = m.get(t.connection_id) ?? { appId: t.app_id, allow: 0, ask: 0, block: 0 };
      if (t.decision === "needs_approval") r.ask += 1;
      else if (t.decision === "allow" || t.decision === "rate_limited") r.allow += 1;
      else r.block += 1;
      m.set(t.connection_id, r);
    }
    return [...m.entries()];
  }, [effective.data]);

  return (
    <Section id="connection-grants" title="Connections" description="What this agent may do through each connection. Only workspace owners change it.">
      {effective.isPending ? <Loading /> : null}
      {effective.isError ? <ErrorState error={effective.error} onRetry={() => effective.refetch()} /> : null}
      {effective.data && rows.length === 0 ? <EmptyState title="This agent has no connection access" /> : null}
      {rows.length > 0 ? (
        <ItemList aria-label="Connection access (read only)">
          {rows.map(([id, r]) => (
            <ListRow key={id}>
              <AppIcon appId={r.appId} name={names.get(r.appId) ?? r.appId} size={32} />
              <div className="min-w-0 flex-1 text-sm">
                <span className="font-medium text-ink">{names.get(r.appId) ?? r.appId}</span>
                <p className="text-xs text-ink-2">
                  {r.allow} allowed, {r.ask} ask, {r.block} blocked
                </p>
              </div>
            </ListRow>
          ))}
        </ItemList>
      ) : null}
    </Section>
  );
}
