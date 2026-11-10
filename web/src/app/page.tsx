"use client";

import { useRouter } from "next/navigation";
import { useEffect } from "react";

import { CreateOrgForm } from "@/components/create-org-form";
import { EmptyState, ErrorState, Loading } from "@/components/states";
import { useMeOnboarding } from "@/lib/api/onboarding";
import { usePlacement } from "@/lib/api/placement";
import { useCreateOrg, useOrgs, useSession, useWorkspaces } from "@/lib/api/queries";
import { useUnauthorizedRedirect } from "@/lib/auth/use-unauthorized-redirect";
import { homeTarget, landingPath } from "@/lib/nav/landing";

export default function Home() {
  useUnauthorizedRedirect();
  const router = useRouter();
  const orgs = useOrgs();
  const firstOrg = orgs.data?.items[0];
  const workspaces = useWorkspaces(firstOrg?.id);
  const create = useCreateOrg();

  const session = useSession();
  const isAdmin = session.data?.user.is_instance_admin === true;
  const placement = usePlacement(isAdmin);
  const onboarding = useMeOnboarding(session.isSuccess);

  const landing = orgs.data
    ? workspaces.isPending && firstOrg
      ? null
      : landingPath(orgs.data.items, workspaces.data?.items)
    : null;
  // Wait for what decides the destination (an admin's placement, the Welcome state) before moving.
  const decided = !session.isPending && (!isAdmin || !placement.isPending) && (!session.isSuccess || !onboarding.isPending);
  const target = decided
    ? homeTarget({
        landing,
        me: session.data,
        isInstanceAdmin: isAdmin,
        placementSaved: placement.data?.saved,
        welcomeSeen: onboarding.data?.welcome_seen,
      })
    : null;

  useEffect(() => {
    if (!target) return;
    // `?tour=1` (the account menu from a page without the navigation) rides along to the landing page.
    const tour = new URLSearchParams(window.location.search).get("tour") === "1";
    router.replace(tour && !target.includes("?") ? `${target}?tour=1` : target);
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
