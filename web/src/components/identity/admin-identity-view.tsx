"use client";

import { grantSummary } from "@/components/identity/idp-roles-list";
import { ErrorState, Loading } from "@/components/states";
import { Badge } from "@/components/ui/badge";
import { DataTable, Td, Th } from "@/components/ui/data-table";
import { Notice } from "@/components/ui/notice";
import { PageHeader } from "@/components/ui/page-header";
import { Section } from "@/components/ui/section";
import { useAdminIdentity } from "@/lib/api/org";

/** Admin › Identity (P4-IDP-1, D-139): the role mapping from the Helm values, read-only. */
export function AdminIdentityView() {
  const q = useAdminIdentity();
  if (q.isPending) return <Loading />;
  if (q.isError) return <ErrorState error={q.error} onRetry={() => q.refetch()} />;
  const providers = q.data.providers;
  return (
    <div className="max-w-page space-y-6">
      <PageHeader
        title="Identity"
        description="Which sign-in role values give which roles. Set in the Helm values (config.auth.oidc_providers), never here."
        className="mb-6"
      />
      {providers.map((p) => (
        <Section
          key={p.id}
          id={`idp-${p.id}`}
          title={p.display_name || p.id}
          description={p.issuer}
        >
          {p.role_claim ? (
            <dl className="mb-3 flex flex-wrap gap-x-8 gap-y-2 text-sm">
              <div>
                <dt className="text-xs font-semibold uppercase tracking-label text-muted">Role claim</dt>
                <dd>
                  <code className="font-mono">{p.role_claim}</code> <span className="text-muted">({p.role_claim_source})</span>
                </dd>
              </div>
              <div>
                <dt className="text-xs font-semibold uppercase tracking-label text-muted">Sign-in</dt>
                <dd>{p.require_role ? "Only people with a mapped role may sign in" : "Everyone the provider admits may sign in"}</dd>
              </div>
            </dl>
          ) : (
            <Notice tone="neutral">This provider does not read role values. Roles are set in the dashboard.</Notice>
          )}
          {p.role_claim && p.role_mappings.length === 0 ? (
            <p className="text-sm text-ink-2">No mappings: role values are shown to people but grant nothing.</p>
          ) : null}
          {p.role_mappings.length > 0 ? (
            <DataTable caption={`Role mappings of ${p.id}`}>
              <thead>
                <tr>
                  <Th>Role value</Th>
                  <Th>Grants</Th>
                </tr>
              </thead>
              <tbody>
                {p.role_mappings.map((m) => (
                  <tr key={m.value}>
                    <Td>
                      <code className="font-mono text-xs [overflow-wrap:anywhere]">{m.value}</code>
                    </Td>
                    <Td>
                      <div className="flex flex-wrap gap-1.5">
                        {grantSummary(m).map((g) => (
                          <Badge key={g} tone={g === "Instance admin" ? "accent" : "neutral"} className="normal-case tracking-normal">
                            {g}
                          </Badge>
                        ))}
                      </div>
                    </Td>
                  </tr>
                ))}
              </tbody>
            </DataTable>
          ) : null}
        </Section>
      ))}
    </div>
  );
}
