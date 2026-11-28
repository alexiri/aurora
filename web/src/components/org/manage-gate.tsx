"use client";

import { useParams, usePathname, useRouter } from "next/navigation";
import { useEffect, type ReactNode } from "react";

import { Loading } from "@/components/states";
import { useMe, useOrg, useResolve } from "@/lib/api/queries";
import { lastWorkspace } from "@/lib/nav/manage-return";
import { wsRoutes } from "@/lib/routes";

/**
 * (D-508(e), C-1178) Members do not see Manage <org>: every Manage address (`/o/<org>` and `/o/<org>/<topic>`, never
 * `/o/<org>/w/…`) sends a member to their last workspace in that organization, else their first, else `/`. Owners,
 * admins and instance admins pass. A navigation decision only: every API route keeps its own authorization. An
 * emergency reader is an organization owner, so the manage page of a workspace stays theirs (FIX-277).
 */
export function ManageGate({ children }: { children: ReactNode }) {
  const { org: slug } = useParams<{ org: string }>();
  const pathname = usePathname() ?? "";
  const manage = !!slug && !/^\/o\/[^/]+\/w\//.test(pathname);
  if (!manage) return <>{children}</>;
  return <Gate slug={slug}>{children}</Gate>;
}

function Gate({ slug, children }: { slug: string; children: ReactNode }) {
  const router = useRouter();
  const me = useMe();
  const resolved = useResolve(slug);
  const org = useOrg(resolved.data?.org.id);
  // `me.data` is `null` when the session cannot be read: such a person is not known to be an instance admin.
  const instanceAdmin = me.data?.user.is_instance_admin === true;
  const member = !me.isPending && !instanceAdmin && org.data?.my_role === "member";
  // A session read before the person joined lists no workspace yet: wait for the one on its way.
  const to = member && !me.isFetching ? target(slug, me.data?.user.id, me.data?.memberships) : null;
  useEffect(() => {
    if (to) router.replace(to);
  }, [to, router]);

  // Nothing of Manage is drawn until both the session and the role are known, whichever answers first.
  if (member) return <Loading />;
  if (instanceAdmin) return <>{children}</>;
  // Unknown or refused: the page shows its own error, or the shell its not-found state.
  if (resolved.isError || org.isError) return <>{children}</>;
  if (me.isPending || !org.data) return <Loading />;
  return <>{children}</>;
}

function target(slug: string, userId: string | undefined, memberships: Parameters<typeof lastWorkspace>[1]): string {
  const w = lastWorkspace(userId, memberships, slug);
  return w ? wsRoutes(slug, w.wsSlug).home() : "/";
}
