"use client";

import { NeedsYouOrg } from "@/components/org/needs-you-org";
import { OrgGate } from "@/components/org/org-gate";
import { PageHeader } from "@/components/ui/page-header";
import { StatLink } from "@/components/ui/stat-tile";
import { useAccounts } from "@/lib/api/accounts";
import { useAllMembers, useOrganizationAgents } from "@/lib/api/org";
import { useWorkspaces, type Org } from "@/lib/api/queries";
import { orgRoutes } from "@/lib/routes";

/** A figure that is not known yet (loading or refused) reads as a dash, never as zero. */
const figure = (n: number | undefined, more = false) => (n === undefined ? "–" : `${n}${more ? "+" : ""}`);

function Summary({ org, slug }: { org: Org; slug: string }) {
  const workspaces = useWorkspaces(org.id);
  const members = useAllMembers(org.id);
  const agents = useOrganizationAgents(org.id);
  const accounts = useAccounts({ kind: "organization", id: org.id });
  const routes = orgRoutes(slug);
  const people = members.data?.pages.flatMap((p) => p.items).length;
  const keys = accounts.data?.items.filter((a) => a.kind === "ai_key").length;
  return (
    <ul aria-label="Summary" className="grid grid-cols-2 gap-3 lg:grid-cols-4">
      <li>
        <StatLink href={routes.workspaces()} label="Workspaces" value={figure(workspaces.data?.items.length)} />
      </li>
      <li>
        <StatLink href={routes.people()} label="People" value={figure(people, members.hasNextPage)} />
      </li>
      <li>
        <StatLink href={routes.agents()} label="Agents" value={figure(agents.data?.items.length)} />
      </li>
      <li>
        <StatLink href={routes.accounts({ kind: "ai-key" })} label="AI keys" value={figure(keys)} />
      </li>
    </ul>
  );
}

/** (D-508, nav-3b) Manage <org> › **Overview**: summary cards, each a link to its page, and what needs the owner. */
export function OrgOverview({ slug }: { slug: string }) {
  return (
    <OrgGate slug={slug} allow={["owner", "admin"]}>
      {(org) => (
        <div className="space-y-6">
          <PageHeader title={org.name} description={`Everything ${org.name}'s workspaces share`} />
          <Summary org={org} slug={slug} />
          <NeedsYouOrg orgId={org.id} orgSlug={slug} />
        </div>
      )}
    </OrgGate>
  );
}
