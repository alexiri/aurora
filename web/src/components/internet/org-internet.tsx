"use client";

import { useState } from "react";

import { EgressTab } from "@/components/governance/egress-tab";
import { InternetLevel, type GovernanceEditor } from "@/components/internet/internet-level";
import { ErrorState, Loading } from "@/components/states";
import { Button } from "@/components/ui/button";
import { LoadMore } from "@/components/ui/load-more";
import { Notice } from "@/components/ui/notice";
import { PageHeader } from "@/components/ui/page-header";
import { ScopeLadder } from "@/components/ui/scope-ladder";
import { Section } from "@/components/ui/section";
import { WhoCan } from "@/components/ui/who-can";
import { errorMessage } from "@/lib/api/errors";
import {
  formFromDocument,
  governanceIssues,
  formToDocument,
  isStale,
  useEffectiveGovernance,
  useGovernanceDoc,
  useInstanceGovernance,
  usePutGovernance,
  type EffectiveGovernance,
  type GovForm,
} from "@/lib/api/governance";
import { useAllMembers } from "@/lib/api/org";
import { overrideValues, useWorkspaceOverrides } from "@/lib/api/overrides";
import { useWorkspaces, type Org } from "@/lib/api/queries";
import { limitsFromAbove } from "@/lib/internet-access";
import { wsRoutes } from "@/lib/routes";
import { whoCanFromContacts } from "@/lib/who-can";

/** The organization's workspaces that tightened Internet access (C-964), as the "below" part of the ladder. */
function useBelow(org: Org, slug: string, all: boolean) {
  const q = useWorkspaceOverrides(org.id, all);
  const workspaces = useWorkspaces(org.id);
  const slugOf = new Map((workspaces.data?.items ?? []).map((w) => [w.id, w.slug]));
  const items = (q.data?.pages ?? []).flatMap((p) => p.items);
  return {
    query: q,
    below: {
      columns: ["Requests without an account", "Browser connections", "Default rule", "Rules"],
      empty: all ? "This organization has no workspaces." : "No workspace restricts this further.",
      rows: items.flatMap((o) => {
        const s = slugOf.get(o.workspace.id);
        if (!s) return [];
        return [
          {
            label: o.workspace.personal_of ? `${o.workspace.personal_of.name} (personal)` : o.workspace.name,
            values: overrideValues(o, org.name),
            link: { href: wsRoutes(slug, s).internetAccess(), label: "Open" },
          },
        ];
      }),
    },
  };
}

/** The governance document's egress part as the organization's controls, with its own pending state for the page's one bar. */
function OrgInternetBody({ org, slug, eff, revision: initialRevision, initial }: { org: Org; slug: string; eff: EffectiveGovernance; revision: number; initial: GovForm }) {
  const [form, setForm] = useState<GovForm>(initial);
  const [baseline, setBaseline] = useState(JSON.stringify(initial));
  const [revision, setRevision] = useState(initialRevision);
  const put = usePutGovernance(org.id);
  const [all, setAll] = useState(false);
  const { query, below } = useBelow(org, slug, all);
  const members = useAllMembers(org.id);
  const users = (members.data?.pages ?? []).flatMap((p) => p.items).map((m) => ({ id: m.user_id, label: `${m.display_name} (${m.email})` }));
  const issues = governanceIssues(put.error);
  const dirty = JSON.stringify(form) !== baseline;

  const governance: GovernanceEditor = {
    dirty,
    apply: async () => {
      try {
        const res = await put.mutateAsync({ document: formToDocument(form), revision });
        setRevision(res.revision);
        setBaseline(JSON.stringify(form));
        put.reset();
        return true;
      } catch {
        return false;
      }
    },
    discard: () => {
      put.reset();
      setForm(JSON.parse(baseline) as GovForm);
    },
    controls: (
      <div className="space-y-4">
        {isStale(put.error) ? (
          <Notice tone="pending" role="alert">
            The limits were changed by someone else. Reload the page to see the latest version.
          </Notice>
        ) : put.isError ? (
          <Notice tone="danger" role="alert">
            {errorMessage(put.error)}
            {issues.general.length ? (
              <ul className="list-disc pl-5">
                {issues.general.map((m, i) => (
                  <li key={i}>{m}</li>
                ))}
              </ul>
            ) : null}
          </Notice>
        ) : null}
        <EgressTab form={form} patch={(p) => setForm((f) => ({ ...f, ...p }))} issues={issues} readOnly={false} eff={eff} orgId={org.id} />
      </div>
    ),
  };

  return (
    <InternetLevel
      level="org"
      scope={{ kind: "organization", id: org.id }}
      orgId={org.id}
      orgSlug={slug}
      orgName={org.name}
      canEdit
      eff={eff}
      whoCanHere={whoCanFromContacts("org_admin", org.contacts, "change_this")}
      whoCanOrg={whoCanFromContacts("org_admin", org.contacts, "change_this")}
      users={users}
      governance={governance}
      below={below}
      belowNote={
        query.isError ? (
          <ErrorState error={query.error} onRetry={() => query.refetch()} />
        ) : (
          <div className="mt-2 flex flex-wrap gap-2">
            <LoadMore query={query} label="Show more" size="sm" />
            <Button variant="ghost" size="sm" aria-pressed={all} onClick={() => setAll((a) => !a)}>
              {all ? "Show only workspaces that differ" : "Show all workspaces"}
            </Button>
          </div>
        )
      }
    />
  );
}

/** (P5-IA-2, C-965) Internet access of an organization: owners and admins edit; members read the limits. */
export function OrgInternet({ org, slug }: { org: Org; slug: string }) {
  const admin = org.my_role === "owner" || org.my_role === "admin";
  const eff = useEffectiveGovernance(org.id);
  const doc = useGovernanceDoc(org.id, admin);
  const instance = useInstanceGovernance(admin);
  if (eff.isPending || (admin && (doc.isPending || instance.isPending))) return <Loading />;
  if (eff.isError) return <ErrorState error={eff.error} onRetry={() => eff.refetch()} />;
  if (admin && doc.isError) return <ErrorState error={doc.error} onRetry={() => doc.refetch()} />;
  if (!admin) {
    const who = whoCanFromContacts("org_admin", org.contacts, "change_this");
    return (
      <div className="max-w-page space-y-6">
        <PageHeader title="Internet access" description="Which websites agents can reach. Only owners and admins can change this." className="mb-2" />
        <Notice role="status">
          <WhoCan value={who} lead className="text-inherit" />
        </Notice>
        <Section id="internet-limits" title="Limits">
          <ScopeLadder above={limitsFromAbove("workspace", eff.data, { org: who, instance: whoCanFromContacts("instance_admin", undefined, "change_this") })} />
        </Section>
      </div>
    );
  }
  return <OrgInternetBody org={org} slug={slug} eff={eff.data} revision={doc.data?.revision ?? 0} initial={formFromDocument(doc.data?.document, eff.data, instance.data)} />;
}
