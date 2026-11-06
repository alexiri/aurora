"use client";

import { useRouter } from "next/navigation";
import { useEffect } from "react";

import { CreateOrgForm } from "@/components/create-org-form";
import { EmptyState, ErrorState, Loading } from "@/components/states";
import { useCreateOrg, useOrgs, useWorkspaces } from "@/lib/api/queries";
import { useUnauthorizedRedirect } from "@/lib/auth/use-unauthorized-redirect";
import { landingPath } from "@/lib/nav/landing";

export default function Home() {
  useUnauthorizedRedirect();
  const router = useRouter();
  const orgs = useOrgs();
  const firstOrg = orgs.data?.items[0];
  const workspaces = useWorkspaces(firstOrg?.id);
  const create = useCreateOrg();

  const target = orgs.data
    ? workspaces.isPending && firstOrg
      ? null
      : landingPath(orgs.data.items, workspaces.data?.items)
    : null;

  useEffect(() => {
    if (target) router.replace(target);
  }, [target, router]);

  let body;
  if (orgs.isPending) body = <Loading />;
  else if (orgs.isError) body = <ErrorState error={orgs.error} onRetry={() => orgs.refetch()} />;
  else if (orgs.data.items.length === 0) {
    body = (
      <EmptyState title="You are not in an organization yet">
        <p className="mb-4">Create one to get started.</p>
        <CreateOrgForm
          pending={create.isPending}
          error={create.error}
          onSubmit={(b) =>
            create.mutate(b, {
              onSuccess: (r) => router.replace(`/o/${r.org.slug}/w/${r.workspace.slug}`),
            })
          }
        />
      </EmptyState>
    );
  } else if (workspaces.isError) {
    body = <ErrorState error={workspaces.error} onRetry={() => workspaces.refetch()} />;
  } else body = <Loading label="Opening your workspace…" />;

  return (
    <main className="mx-auto max-w-xl p-8">
      <h1 className="mb-6 text-2xl font-semibold">Aurora</h1>
      {body}
    </main>
  );
}
