"use client";

import Link from "next/link";
import { useParams } from "next/navigation";

import { CreateWorkspaceDialog } from "@/components/create-workspace-dialog";
import { EmptyState, ErrorState, Loading } from "@/components/states";
import { useResolve, useWorkspaces } from "@/lib/api/queries";

export default function OrgPage() {
  const { org } = useParams<{ org: string }>();
  const resolved = useResolve(org);
  const workspaces = useWorkspaces(resolved.data?.org.id);

  if (resolved.isPending) return <Loading />;
  if (resolved.isError) return <ErrorState error={resolved.error} onRetry={() => resolved.refetch()} />;

  return (
    <div className="space-y-4">
      <div className="flex items-center justify-between">
        <h1 className="text-xl font-semibold">{resolved.data.org.name}</h1>
        <CreateWorkspaceDialog orgId={resolved.data.org.id} orgSlug={org} />
      </div>
      <h2 className="text-sm font-medium text-neutral-600">Workspaces</h2>
      {workspaces.isPending ? <Loading /> : null}
      {workspaces.isError ? (
        <ErrorState error={workspaces.error} onRetry={() => workspaces.refetch()} />
      ) : null}
      {workspaces.data?.items.length === 0 ? (
        <EmptyState title="No workspaces">Create one to get started.</EmptyState>
      ) : null}
      {workspaces.data && workspaces.data.items.length > 0 ? (
        <ul className="divide-y divide-neutral-200 rounded-md border border-neutral-200">
          {workspaces.data.items.map((w) => (
            <li key={w.id}>
              <Link
                href={`/o/${org}/w/${w.slug}`}
                className="flex items-center justify-between p-3 hover:bg-neutral-50"
              >
                <span className="font-medium">{w.name}</span>
                <span className="font-mono text-sm text-neutral-500">{w.slug}</span>
              </Link>
            </li>
          ))}
        </ul>
      ) : null}
    </div>
  );
}
