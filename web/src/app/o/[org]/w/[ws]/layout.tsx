"use client";

// (FIX-235, D-477) An old workspace address redirects an administrator who is not one of the
// workspace's people to the manage page: every page under `/o/<org>/w/<ws>` would refuse them
// (403 `workspace_access_required`), and the manage page is where their controls live.

import Link from "next/link";
import { useEffect, type ReactNode } from "react";
import { useParams, usePathname, useRouter } from "next/navigation";

import { Loading } from "@/components/states";
import { Notice } from "@/components/ui/notice";
import { useResolve, useWorkspace } from "@/lib/api/queries";
import { emergencyCovers, isEmergencyReader } from "@/lib/emergency";
import { orgRoutes, wsRoutes } from "@/lib/routes";

export default function WorkspaceRedirect({ children }: { children: ReactNode }) {
  const { org, ws } = useParams<{ org: string; ws: string }>();
  const router = useRouter();
  const pathname = usePathname() ?? "";
  const resolved = useResolve(org, ws);
  const wsId = resolved.data?.workspace?.id;
  const workspace = useWorkspace(wsId);
  const w = workspace.data;
  // (FIX-277, D-478) Never the emergency reader: an organization owner with an open grant also `administers`, and
  // the pages here are what the grant lets them read. When it ends, the next answer sends them to the manage page.
  const reader = isEmergencyReader(w);
  const manage = !!w && w.my_role == null && w.administers === true && !reader;
  useEffect(() => {
    if (manage) router.replace(orgRoutes(org).workspace(ws));
  }, [manage, router, org, ws]);
  if (manage) return <Loading />;
  // (FIX-277) A page the grant doesn't cover never mounts for a reader, so it can't offer to act or ask for what would
  // be refused. It waits for the workspace answer to know; the covered pages wait for it themselves.
  const rest = pathname.split("/").slice(5).join("/");
  if (!emergencyCovers(rest)) {
    if (wsId && workspace.isPending) return <Loading />;
    if (reader) {
      return (
        <Notice role="note" data-testid="emergency-not-covered">
          Emergency read access doesn&rsquo;t cover this page.{" "}
          <Link href={wsRoutes(org, ws).agents()} className="font-medium text-link underline">
            Read the workspace&rsquo;s agents
          </Link>
        </Notice>
      );
    }
  }
  return <>{children}</>;
}
