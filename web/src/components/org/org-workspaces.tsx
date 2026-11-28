"use client";

import { useQueries } from "@tanstack/react-query";
import Link from "next/link";

import { CreateWorkspaceDialog } from "@/components/create-workspace-dialog";
import { OrgGate } from "@/components/org/org-gate";
import { EmptyState, ErrorState, Loading } from "@/components/states";
import { PageHeader } from "@/components/ui/page-header";
import { ResponsiveTable } from "@/components/ui/responsive-table";
import { When } from "@/components/ui/when";
import { api } from "@/lib/api/client";
import { useOrganizationAgents } from "@/lib/api/org";
import { queryKeys, useMe, useWorkspaces, type Org, type Workspace } from "@/lib/api/queries";
import { unwrap } from "@/lib/api/unwrap";
import { orgRoutes, wsRoutes } from "@/lib/routes";
import { personName, workspaceDisplayName } from "@/lib/workspace-name";

/** "Ada Lovelace, Grace Hopper +2": the names the workspace's own answer carries (at most five). */
export function ownersLine(audience: NonNullable<Workspace["audience"]>): string {
  if (audience.owners_total === 0) return "No owner";
  const names = audience.owners.map((o) => personName(o.display_name)).join(", ");
  const more = audience.owners_total - audience.owners.length;
  return more > 0 ? `${names} +${more}` : names;
}

/**
 * A team workspace's owners. The organization's list does not carry them, so each one is read from the workspace itself
 * (the same query its pages use); a personal workspace's owner is its person.
 */
function useTeamOwners(workspaces: Workspace[]): Map<string, string> {
  const team = workspaces.filter((w) => !w.personal_of);
  const answers = useQueries({
    queries: team.map((w) => ({
      queryKey: queryKeys.workspace(w.id),
      queryFn: () => unwrap(api.GET("/v1/workspaces/{ws}", { params: { path: { ws: w.id } } })),
      retry: false,
    })),
  });
  return new Map(
    team.map((w, i) => {
      const a = answers[i]!;
      return [w.id, a.data?.audience ? ownersLine(a.data.audience) : a.isPending ? "…" : "—"];
    }),
  );
}

function List({ org, slug }: { org: Org; slug: string }) {
  const viewer = useMe().data?.user;
  const workspaces = useWorkspaces(org.id);
  const agents = useOrganizationAgents(org.id);
  const items = workspaces.data?.items ?? [];
  const owners = useTeamOwners(items);
  if (workspaces.isPending) return <Loading variant="rows" label="Loading workspaces…" />;
  if (workspaces.isError) return <ErrorState error={workspaces.error} onRetry={() => workspaces.refetch()} />;
  if (items.length === 0) return <EmptyState title="No workspaces">Create one to get started.</EmptyState>;
  // (P5-POLISH-9) Team workspaces first A to Z, then personal ones by their owner's name.
  const rows = [...items].sort(
    (a, b) =>
      Number(!!a.personal_of) - Number(!!b.personal_of) ||
      (a.personal_of && b.personal_of
        ? a.personal_of.name.localeCompare(b.personal_of.name)
        : workspaceDisplayName(a, viewer).localeCompare(workspaceDisplayName(b, viewer))),
  );
  const agentCount = (id: string) => (agents.data ? agents.data.items.filter((a) => a.workspace.id === id).length : agents.isPending ? "…" : "—");
  return (
    <ResponsiveTable
      caption="Workspaces"
      title={{
        header: "Name",
        cell: (w) => (
          // (FIX-235, D-477) A workspace this person is in opens as usual; an administrator who is not one of its people
          // gets the manage page, with a quiet "Manage" label.
          <Link href={w.my_role != null ? wsRoutes(slug, w.slug).home() : orgRoutes(slug).workspace(w.slug)} className="font-medium text-ink underline-offset-2 hover:underline">
            {workspaceDisplayName(w, viewer)}
            {w.my_role == null ? (
              <>
                {" "}
                <span className="ml-1 text-sm font-normal text-muted" data-testid="manage-row">
                  Manage
                </span>
              </>
            ) : null}
          </Link>
        ),
      }}
      columns={[
        { key: "kind", header: "Kind", cell: (w) => (w.personal_of ? "Personal" : "Team") },
        { key: "owners", header: "Owners", cell: (w) => (w.personal_of ? personName(w.personal_of.name) : owners.get(w.id)) },
        { key: "agents", header: "Agents", cell: (w) => <span className="tabular-nums">{agentCount(w.id)}</span> },
        { key: "created", header: "Created", cell: (w) => <When value={w.created_at} /> },
      ]}
      rows={rows}
      rowKey={(w) => w.id}
    />
  );
}

/** (D-508(c), C-1178) Manage <org> › **Workspaces**: every workspace of the organization, with New workspace. */
export function OrgWorkspaces({ slug }: { slug: string }) {
  return (
    <OrgGate slug={slug} allow={["owner", "admin"]}>
      {(org) => (
        <>
          <PageHeader
            title={`Workspaces in ${org.name}`}
            description="Where the organization's people and agents work."
            actions={<CreateWorkspaceDialog orgId={org.id} orgSlug={slug} />}
          />
          <List org={org} slug={slug} />
        </>
      )}
    </OrgGate>
  );
}
