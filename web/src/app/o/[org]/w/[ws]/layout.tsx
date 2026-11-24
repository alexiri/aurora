"use client";

// (FIX-235, D-477) An old workspace address redirects an administrator who is not one of the
// workspace's people to the manage page: every page under `/o/<org>/w/<ws>` would refuse them
// (403 `workspace_access_required`), and the manage page is where their controls live.

import { useEffect, type ReactNode } from "react";
import { useParams, useRouter } from "next/navigation";

import { Loading } from "@/components/states";
import { useResolve, useWorkspace } from "@/lib/api/queries";
import { orgRoutes } from "@/lib/routes";

export default function WorkspaceRedirect({ children }: { children: ReactNode }) {
  const { org, ws } = useParams<{ org: string; ws: string }>();
  const router = useRouter();
  const resolved = useResolve(org, ws);
  const wsId = resolved.data?.workspace?.id;
  const workspace = useWorkspace(wsId);
  const w = workspace.data;
  const manage = !!w && w.my_role == null && w.administers === true;
  useEffect(() => {
    if (manage) router.replace(orgRoutes(org).workspace(ws));
  }, [manage, router, org, ws]);
  if (manage) return <Loading />;
  return <>{children}</>;
}
