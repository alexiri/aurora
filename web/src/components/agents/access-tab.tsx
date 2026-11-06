"use client";

import { EmptyState, ErrorState, Loading } from "@/components/states";
import { useEffective, useGrants, useToggleGrant } from "@/lib/api/policy";
import { useSecrets } from "@/lib/api/secrets";

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
    <section aria-label="Secret grants" className="space-y-2">
      <h2 className="text-lg font-medium">Secrets</h2>
      {toggle.isError ? <ErrorState error={toggle.error} /> : null}
      {secrets.length === 0 ? (
        <EmptyState title="No secrets in scope" />
      ) : (
        <ul className="space-y-1">
          {secrets.map((s) => (
            <li key={s.id} className="flex items-center justify-between rounded border border-neutral-200 p-2 text-sm">
              <label className="flex items-center gap-2">
                <input
                  type="checkbox"
                  checked={granted.has(s.id)}
                  disabled={toggle.isPending}
                  onChange={(e) => toggle.mutate({ secretId: s.id, grant: e.target.checked })}
                  aria-label={`Grant ${s.name}`}
                />
                <span className="font-medium">{s.name}</span>
              </label>
              <span className="text-xs text-neutral-600">
                {s.scope} · {s.kind === "llm" ? "LLM key" : "secret"}
              </span>
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}

export function AccessTab({ agentId, wsId, orgId, canOwn }: { agentId: string; wsId: string; orgId: string; canOwn: boolean }) {
  const effective = useEffective(agentId);

  return (
    <div className="space-y-6">
      {canOwn ? <SecretGrants agentId={agentId} wsId={wsId} orgId={orgId} /> : null}

      <section aria-label="Effective access" className="space-y-2">
        <h2 className="text-lg font-medium">Effective access</h2>
        {effective.isPending ? <Loading /> : null}
        {effective.isError ? <ErrorState error={effective.error} onRetry={() => effective.refetch()} /> : null}
        {effective.data && effective.data.credentials.length === 0 ? (
          <p className="text-sm text-neutral-600">No credentials are available to this agent.</p>
        ) : null}
        <ul className="space-y-1 text-sm">
          {effective.data?.credentials.map((c) => (
            <li key={c.id} className="rounded border border-neutral-200 p-2">
              <span className="font-medium">{c.name}</span> <span className="text-xs text-neutral-600">({c.kind}, {c.scope})</span>{" "}
              {c.selected ? <strong>selected</strong> : <span className="text-neutral-500">not selected</span>}
              {c.selected_by.length ? <span className="text-xs text-neutral-600"> · selected by {c.selected_by.length} rule(s): {c.selected_by.join(", ")}</span> : null}
            </li>
          ))}
        </ul>
      </section>
    </div>
  );
}
