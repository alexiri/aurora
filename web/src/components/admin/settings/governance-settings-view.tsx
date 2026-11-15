"use client";

import Link from "next/link";
import { useState } from "react";

import { SettingsSectionForm, type SettingsGroup } from "@/components/admin/settings/settings-section-form";
import { ErrorState, Loading } from "@/components/states";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { DataTable, Td, Th } from "@/components/ui/data-table";
import { FormSection } from "@/components/ui/form";
import { Notice } from "@/components/ui/notice";
import { PageHeader } from "@/components/ui/page-header";
import { useGovernanceOrgs, useSettingsSection, type GovernanceOrgRow } from "@/lib/api/settings";

/** The four groups of the page (`08 §11`); every key of the registry's `governance` section appears once. */
export const GOVERNANCE_GROUPS: SettingsGroup[] = [
  {
    title: "LLM",
    description:
      "The floor every organization's model governance stays above: which key scopes may hold provider keys, the minimum posture, and whether a key's origin must be verified (08 §11.1).",
    keys: [
      "governance.llm.minimum_posture",
      "governance.llm.allowed_key_scopes",
      "governance.llm.require_key_origin",
      "governance.llm.known_llm_hosts_extra",
    ],
  },
  {
    title: "Egress",
    description:
      "The baseline for what agents may reach: the minimum egress posture, the hosts allowed everywhere, and how far the open lane may be opened.",
    keys: [
      "governance.egress.minimum_posture",
      "governance.egress.baseline_allowlist",
      "governance.egress.open_lane_minimum",
    ],
  },
  {
    title: "BYO agents",
    description:
      "Agents that run outside Aurora. The switch is live: turning it off stops new BYO agents and refuses their credentials at the gateway at once.",
    keys: [
      "byo.enabled",
      "byo.minimum_posture",
      "byo.default_source_cidrs",
      "byo.require_context_credentials",
    ],
  },
  {
    title: "Agent tokens",
    description:
      "The longest life of an agent token. Lowering it shortens live tokens, keeping at least seven days; raising or removing it never extends one (08 §7).",
    keys: ["governance.agent_tokens.max_age_days"],
  },
];

function label(v: string): string {
  return v.replace(/_/g, " ");
}

/** Admin › Settings › Governance (SET-2, C-778): the generic form in four sections, then where organizations stand. */
export function GovernanceSettingsView() {
  const q = useSettingsSection("governance");
  if (q.isPending) return <Loading />;
  if (q.isError) return <ErrorState error={q.error} onRetry={() => q.refetch()} />;
  const section = q.data;
  return (
    <div className="max-w-page space-y-6">
      <PageHeader
        breadcrumb={[{ label: "Settings", href: "/admin/settings" }, { label: section.title }]}
        title={section.title}
        description={section.description}
      />
      <Notice tone="neutral" role="status" className="max-w-3xl">
        Policy is set per organization and workspace. There is no instance-wide rule scope: this page sets the
        floor those policies cannot go below. See docs/operator/gateway-posture.md.
      </Notice>
      <SettingsSectionForm
        key={section.generation}
        section={section}
        groups={GOVERNANCE_GROUPS}
        onReload={() => void q.refetch()}
      />
      <OrgsStanding />
    </div>
  );
}

/** "Where organizations stand": effective postures and where an organization is stricter or clamped. */
function OrgsStanding() {
  const [cursors, setCursors] = useState<string[]>([]);
  const cursor = cursors[cursors.length - 1];
  const q = useGovernanceOrgs(cursor);
  return (
    <section aria-labelledby="orgs-standing" className="max-w-5xl space-y-3">
      <FormSection title="Where organizations stand" description="Each organization's effective postures under the settings above. Stricter: the organization's own value is tighter than the floor. Clamped: its own value is looser and the floor overrides it.">
        <span id="orgs-standing" className="sr-only">
          Where organizations stand
        </span>
        {q.isPending ? <Loading /> : null}
        {q.isError ? <ErrorState error={q.error} onRetry={() => q.refetch()} /> : null}
        {q.data ? (
          <>
            <DataTable caption="Organizations and their effective governance">
              <thead>
                <tr>
                  <Th>Organization</Th>
                  <Th>LLM</Th>
                  <Th>Egress</Th>
                  <Th>Open lane</Th>
                  <Th>BYO</Th>
                  <Th>Stricter</Th>
                  <Th>Clamped</Th>
                </tr>
              </thead>
              <tbody>
                {q.data.items.length === 0 ? (
                  <tr>
                    <Td colSpan={7} className="text-ink-2">
                      No organizations yet.
                    </Td>
                  </tr>
                ) : null}
                {q.data.items.map((o: GovernanceOrgRow) => (
                  <tr key={o.org_id}>
                    <Td>
                      <Link className="font-medium text-ink underline-offset-2 hover:underline" href={`/o/${o.slug}/governance`}>
                        {o.name}
                      </Link>
                      {o.frozen ? <Badge className="ml-2" tone="pending">frozen</Badge> : null}
                    </Td>
                    <Td>{label(o.llm_posture)}</Td>
                    <Td>{label(o.egress_posture)}</Td>
                    <Td>{label(o.open_lane)}</Td>
                    <Td>{label(o.byo_posture)}</Td>
                    <Td>{o.stricter.length > 0 ? o.stricter.map((k) => <div key={k}><code className="font-mono text-xs">{k}</code></div>) : <span className="text-muted">—</span>}</Td>
                    <Td>{o.clamped.length > 0 ? o.clamped.map((k) => <div key={k}><code className="font-mono text-xs">{k}</code></div>) : <span className="text-muted">—</span>}</Td>
                  </tr>
                ))}
              </tbody>
            </DataTable>
            <div className="flex flex-wrap gap-2">
              <Button type="button" variant="outline" size="sm" disabled={cursors.length === 0} onClick={() => setCursors((c) => c.slice(0, -1))}>
                Newer
              </Button>
              <Button
                type="button"
                variant="outline"
                size="sm"
                disabled={!q.data.next_cursor}
                onClick={() => q.data.next_cursor && setCursors((c) => [...c, q.data.next_cursor as string])}
              >
                Older
              </Button>
            </div>
          </>
        ) : null}
      </FormSection>
    </section>
  );
}
