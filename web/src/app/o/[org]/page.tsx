"use client";

import Link from "next/link";
import { useParams } from "next/navigation";

import { CreateWorkspaceDialog } from "@/components/create-workspace-dialog";
import { EmptyState, ErrorState, Loading } from "@/components/states";
import { PageHeader } from "@/components/ui/page-header";
import { ItemList, ListRow, Section } from "@/components/ui/section";
import { useMe, useResolve, useWorkspaces } from "@/lib/api/queries";
import { workspaceDisplayName } from "@/lib/workspace-name";

export default function OrgPage() {
  const { org } = useParams<{ org: string }>();
  const me = useMe();
  const resolved = useResolve(org);
  const workspaces = useWorkspaces(resolved.data?.org.id);

  if (resolved.isPending) return <Loading />;
  if (resolved.isError) return <ErrorState error={resolved.error} onRetry={() => resolved.refetch()} />;
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
    <div className="max-w-page">
      <PageHeader
        title={resolved.data.org.name}
        description="Workspaces in this organization."
        actions={<CreateWorkspaceDialog orgId={resolved.data.org.id} orgSlug={org} />}
      />
      <Section id="org-workspaces" title="Workspaces">
        {workspaces.isPending ? <Loading /> : null}
        {workspaces.isError ? (
          <ErrorState error={workspaces.error} onRetry={() => workspaces.refetch()} />
        ) : null}
        {workspaces.data?.items.length === 0 ? (
          <EmptyState title="No workspaces">Create one to get started.</EmptyState>
        ) : null}
        {shared.length > 0 ? <ItemList aria-label="Workspaces">{shared.map(row)}</ItemList> : null}
      </Section>
      {/* (P5-POLISH-9) Shared workspaces first; everyone's personal workspace under its own heading, with its owner's name. */}
      {personal.length > 0 ? (
        <Section id="org-personal-workspaces" title="Personal workspaces">
          <ItemList aria-label="Personal workspaces">{personal.map(row)}</ItemList>
        </Section>
      ) : null}
    </div>
  );
}
