"use client";

import Link from "next/link";
import { useParams } from "next/navigation";

import { CreateWorkspaceDialog } from "@/components/create-workspace-dialog";
import { EmptyState, ErrorState, Loading } from "@/components/states";
import { PageHeader } from "@/components/ui/page-header";
import { ItemList, ListRow, Section } from "@/components/ui/section";
import { useResolve, useWorkspaces } from "@/lib/api/queries";

export default function OrgPage() {
  const { org } = useParams<{ org: string }>();
  const resolved = useResolve(org);
  const workspaces = useWorkspaces(resolved.data?.org.id);

  if (resolved.isPending) return <Loading />;
  if (resolved.isError) return <ErrorState error={resolved.error} onRetry={() => resolved.refetch()} />;

  return (
    <div className="max-w-[1160px]">
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
        {workspaces.data && workspaces.data.items.length > 0 ? (
          <ItemList aria-label="Workspaces">
            {workspaces.data.items.map((w) => (
              <ListRow key={w.id} className="p-0 sm:px-0">
                <Link
                  href={`/o/${org}/w/${w.slug}`}
                  className="flex min-w-0 flex-1 flex-wrap items-center justify-between gap-x-3 gap-y-1 px-4 py-3 hover:bg-subtle"
                >
                  <span className="font-medium text-ink">{w.name}</span>
                  <span className="font-mono text-sm text-muted">{w.slug}</span>
                </Link>
              </ListRow>
            ))}
          </ItemList>
        ) : null}
      </Section>
    </div>
  );
}
