"use client";

import { EmptyState, ErrorState, Loading } from "@/components/states";
import { Badge } from "@/components/ui/badge";
import { ItemList, ListRow, Section } from "@/components/ui/section";
import { useEffective, useGrants, useToggleGrant } from "@/lib/api/policy";
import { useSecrets } from "@/lib/api/secrets";

import { ConnectionGrants, ConnectionGrantsReadOnly } from "./connection-grants";
import { ModelReadinessSection } from "./model-readiness";

/** Secret grants: workspace owners only (`GET …/grants` and the secret lists are `can_own`, I-27). */
function SecretGrants({ agentId, wsId, orgId }: { agentId: string; wsId: string; orgId: string }) {
  const wsSecrets = useSecrets({ kind: "workspace", id: wsId });
  const orgSecrets = useSecrets({ kind: "organization", id: orgId });
  const grants = useGrants(agentId);
  const toggle = useToggleGrant(agentId);

  if (wsSecrets.isPending || grants.isPending) return <Loading />;
  if (wsSecrets.isError) return <ErrorState error={wsSecrets.error} onRetry={() => wsSecrets.refetch()} />;
  if (grants.isError) return <ErrorState error={grants.error} onRetry={() => grants.refetch()} />;

  const granted = new Set(grants.data.secrets.map((g) => g.secret_id));
  const secrets = [...wsSecrets.data.items, ...(orgSecrets.data?.items ?? [])].filter((s) => s.status === "active");

  return (
    <Section id="secret-grants" title="Secrets" description="Which workspace and organization secrets this agent may use.">
      {toggle.isError ? <ErrorState error={toggle.error} /> : null}
      {secrets.length === 0 ? (
        <EmptyState title="No secrets in scope" />
      ) : (
        <ItemList aria-label="Secret grants">
          {secrets.map((s) => (
            <ListRow key={s.id} actions={<span className="text-xs text-muted">{s.scope} · {s.kind === "llm" ? "LLM key" : "secret"}</span>}>
              <label className="flex min-h-9 items-center gap-2.5 text-sm">
                <input
                  type="checkbox"
                  className="size-4 shrink-0 accent-accent"
                  checked={granted.has(s.id)}
                  disabled={toggle.isPending}
                  onChange={(e) => toggle.mutate({ secretId: s.id, grant: e.target.checked })}
                  aria-label={`Grant ${s.name}`}
                />
                <span className="font-medium text-ink">{s.name}</span>
              </label>
            </ListRow>
          ))}
        </ItemList>
      )}
    </Section>
  );
}

export function AccessTab({
  agentId,
  wsId,
  orgId,
  canOwn,
  hosted = false,
  base = "",
}: {
  agentId: string;
  wsId: string;
  orgId: string;
  canOwn: boolean;
  /** Hosted agents get the Model readiness section (owners only). */
  hosted?: boolean;
  /** `/o/{org}/w/{ws}`, for the links of the readiness fixes. */
  base?: string;
}) {
  const effective = useEffective(agentId);

  return (
    <div className="space-y-8">
      {canOwn && hosted ? <ModelReadinessSection agentId={agentId} base={base} /> : null}
      {canOwn ? <ConnectionGrants agentId={agentId} wsId={wsId} orgId={orgId} /> : <ConnectionGrantsReadOnly agentId={agentId} wsId={wsId} />}
      {canOwn ? <SecretGrants agentId={agentId} wsId={wsId} orgId={orgId} /> : null}

      <Section id="effective-access" title="Effective access" description="The credentials this agent can use right now, and why.">
        {effective.isPending ? <Loading /> : null}
        {effective.isError ? <ErrorState error={effective.error} onRetry={() => effective.refetch()} /> : null}
        {effective.data && effective.data.credentials.length === 0 ? (
          <EmptyState title="No credentials are available to this agent." />
        ) : null}
        {effective.data && effective.data.credentials.length > 0 ? (
          <ItemList aria-label="Effective credentials">
            {effective.data.credentials.map((c) => (
              <ListRow key={c.id}>
                <div className="min-w-0 flex-1 space-y-1 text-sm">
                  <div className="flex flex-wrap items-center gap-2">
                    <span className="font-medium text-ink">{c.name}</span>
                    <span className="text-xs text-muted">({c.kind === "llm" ? "LLM key" : c.kind}, {c.scope})</span>{" "}
                    {c.selected ? <Badge tone="accent">selected</Badge> : <span className="text-muted">not selected</span>}
                  </div>
                  {c.selected_by.length ? (
                    <p className="text-xs text-muted">selected by {c.selected_by.length} rule(s): {c.selected_by.join(", ")}</p>
                  ) : null}
                  {c.depends_on_speaker ? (
                    <p className="text-xs text-pending-ink">depends on who is speaking (shown for you)</p>
                  ) : null}
                </div>
              </ListRow>
            ))}
          </ItemList>
        ) : null}
      </Section>
    </div>
  );
}
