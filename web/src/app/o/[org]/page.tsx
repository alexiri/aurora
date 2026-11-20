"use client";

import Link from "next/link";
import { useParams } from "next/navigation";

import { CreateWorkspaceDialog } from "@/components/create-workspace-dialog";
import { InviteDialog } from "@/components/org/invite-dialog";
import { peopleLine } from "@/components/org/members-view";
import { AgentRow, useOrgAgents } from "@/components/org/org-agents-list";
import { SetupRequestItem } from "@/components/overview/setup-request-row";
import { EmptyState, ErrorState, Loading } from "@/components/states";
import { buttonVariants } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { PageHeader } from "@/components/ui/page-header";
import { ItemList, ListRow, Section } from "@/components/ui/section";
import { useOrgSetupRequests } from "@/lib/api/app-setup-requests";
import { useAllMembers, useInvitations } from "@/lib/api/org";
import { useMe, useOrg, useResolve, useWorkspaces } from "@/lib/api/queries";
import { orgRoutes } from "@/lib/routes";
import { cn } from "@/lib/utils";
import { workspaceDisplayName } from "@/lib/workspace-name";

function NeedsYouOrg({ orgId, orgSlug }: { orgId: string; orgSlug: string }) {
  const setups = useOrgSetupRequests(orgId, true);
  const invites = useInvitations(orgId);
  const requests = setups.data ?? [];
  const waiting = invites.data?.items.length ?? 0;
  if (requests.length === 0 && waiting === 0) return null;
  return (
    <section aria-labelledby="org-needs-you-h" className="rounded-card border border-pending-line bg-pending-panel p-4 text-ink sm:p-5">
      <h2 id="org-needs-you-h" className="font-display text-base font-semibold text-pending-ink">
        Needs you
      </h2>
      <ul className="mt-3 space-y-2" aria-label="Waiting for you">
        {requests.map((r) => (
          <SetupRequestItem key={r.id} request={r} orgBase={orgRoutes(orgSlug).base} orgId={orgId} />
        ))}
        {waiting > 0 ? (
          <li className="flex flex-wrap items-center gap-x-3 gap-y-1 rounded-control bg-surface px-3 py-2 text-sm">
            <span className="min-w-0 flex-1">
              {waiting} {waiting === 1 ? "invitation is" : "invitations are"} waiting for an answer.
            </span>
            <Link href={`${orgRoutes(orgSlug).base}/people`} className={cn(buttonVariants({ variant: "outline", size: "sm" }))}>
              See invitations
            </Link>
          </li>
        ) : null}
      </ul>
    </section>
  );
}

function PeopleCard({ orgId, orgSlug }: { orgId: string; orgSlug: string }) {
  const members = useAllMembers(orgId);
  const invites = useInvitations(orgId);
  const people = members.data?.pages.flatMap((p) => p.items).length ?? 0;
  return (
    <Section id="org-people-summary" title="People" actions={<InviteDialog orgId={orgId} />}>
      <Card className="p-4 text-sm">
        {members.isPending ? (
          <Loading />
        ) : (
          <Link href={`${orgRoutes(orgSlug).base}/people`} className="font-medium text-ink underline-offset-2 hover:underline">
            {peopleLine(people, invites.data?.items.length ?? 0)}
          </Link>
        )}
      </Card>
    </Section>
  );
}

function AgentsCard({ orgId, orgSlug }: { orgId: string; orgSlug: string }) {
  const { rows, pending } = useOrgAgents(orgId);
  const last = [...rows].sort((a, b) => b.agent.updated_at.localeCompare(a.agent.updated_at)).slice(0, 5);
  return (
    <Section
      id="org-agents-summary"
      title="Agents"
      actions={
        <Link href={orgRoutes(orgSlug).agents()} className={cn(buttonVariants({ variant: "outline", size: "sm" }))}>
          All agents
        </Link>
      }
    >
      {pending && last.length === 0 ? <Loading /> : null}
      {!pending && last.length === 0 ? <EmptyState title="No agents yet" /> : null}
      {last.length > 0 ? (
        <Card className="overflow-hidden p-0">
          <ul className="divide-y divide-line" aria-label="Latest agents">
            {last.map((r) => (
              <AgentRow key={r.agent.id} row={r} orgSlug={orgSlug} />
            ))}
          </ul>
        </Card>
      ) : null}
    </Section>
  );
}

/** Manage <org> › **Overview** (rec 8, rec 7): what needs you, people, workspaces, agents, and the two rules that apply. */
export default function OrgPage() {
  const { org } = useParams<{ org: string }>();
  const me = useMe();
  const resolved = useResolve(org);
  const orgData = useOrg(resolved.data?.org.id);
  const workspaces = useWorkspaces(resolved.data?.org.id);

  if (resolved.isPending) return <Loading />;
  if (resolved.isError) return <ErrorState error={resolved.error} onRetry={() => resolved.refetch()} />;
  const orgId = resolved.data.org.id;
  const role = orgData.data?.my_role;
  const admin = role === "owner" || role === "admin" || me.data?.user.is_instance_admin === true;
  const items = workspaces.data?.items ?? [];
  const shared = items.filter((w) => !w.personal_of).sort((a, b) => workspaceDisplayName(a, me.data?.user).localeCompare(workspaceDisplayName(b, me.data?.user)));
  const personal = items.filter((w) => !!w.personal_of).sort((a, b) => (a.personal_of?.name ?? "").localeCompare(b.personal_of?.name ?? ""));
  const row = (w: (typeof items)[number]) => (
    <ListRow key={w.id} className="p-0 sm:px-0">
      <Link href={`/o/${org}/w/${w.slug}`} className="flex min-w-0 flex-1 flex-wrap items-center justify-between gap-x-3 gap-y-1 px-4 py-3 hover:bg-subtle">
        <span className="font-medium text-ink">
          {workspaceDisplayName(w, me.data?.user)}
          {w.personal_of ? <span className="ml-2 font-normal text-ink-2">{w.personal_of.name}</span> : null}
        </span>
      </Link>
    </ListRow>
  );

  return (
    <div className="max-w-page space-y-6">
      <PageHeader title="Overview" description={`What is going on in ${resolved.data.org.name}.`} className="mb-2" />
      {admin ? <NeedsYouOrg orgId={orgId} orgSlug={org} /> : null}
      {admin ? <PeopleCard orgId={orgId} orgSlug={org} /> : null}
      <Section id="org-workspaces" title="Workspaces" actions={admin ? <CreateWorkspaceDialog orgId={orgId} orgSlug={org} /> : null}>
        {workspaces.isPending ? <Loading /> : null}
        {workspaces.isError ? <ErrorState error={workspaces.error} onRetry={() => workspaces.refetch()} /> : null}
        {workspaces.data?.items.length === 0 ? <EmptyState title="No workspaces">Create one to get started.</EmptyState> : null}
        {shared.length > 0 ? <ItemList aria-label="Workspaces">{shared.map(row)}</ItemList> : null}
      </Section>
      {/* (P5-POLISH-9) Shared workspaces first; everyone's personal workspace under its own heading, with its owner's name. */}
      {personal.length > 0 ? (
        <Section id="org-personal-workspaces" title="Personal workspaces">
          <ItemList aria-label="Personal workspaces">{personal.map(row)}</ItemList>
        </Section>
      ) : null}
      {admin ? <AgentsCard orgId={orgId} orgSlug={org} /> : null}
      <Section id="org-rules" title="Rules that apply">
        <ItemList aria-label="Rules that apply">
          <ListRow className="p-0 sm:px-0">
            <Link href={`/o/${org}/ai-models`} className="block min-w-0 flex-1 px-4 py-3 hover:bg-subtle">
              <span className="font-medium text-ink">AI models</span>
              <span className="ml-2 text-ink-2">Which AI models the agents of {resolved.data.org.name} may use.</span>
            </Link>
          </ListRow>
          <ListRow className="p-0 sm:px-0">
            <Link href={`/o/${org}/internet-access`} className="block min-w-0 flex-1 px-4 py-3 hover:bg-subtle">
              <span className="font-medium text-ink">Internet access</span>
              <span className="ml-2 text-ink-2">What agents in {resolved.data.org.name} may reach.</span>
            </Link>
          </ListRow>
        </ItemList>
      </Section>
    </div>
  );
}
