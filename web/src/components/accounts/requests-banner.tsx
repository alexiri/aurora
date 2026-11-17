"use client";

import Link from "next/link";
import { useParams } from "next/navigation";

import { Button } from "@/components/ui/button";
import { Notice } from "@/components/ui/notice";
import { useAccessRequests } from "@/lib/api/access-requests";
import { wsRoutes } from "@/lib/routes";

/**
 * (P5-UX-17, C-984) The slot above the workspace's Accounts list: each open request of an agent for an app, "Daneel
 * asked for Linear", with **Give Daneel Linear** (the journey for that agent and app, C-959) for people who can
 * change access. A proposed website is listed under Requests below the list. Nothing when nothing is open.
 */
export function RequestsBanner({ wsId, canOwn }: { wsId: string; canOwn: boolean }) {
  const params = useParams<{ org?: string; ws?: string }>();
  const requests = useAccessRequests(wsId);
  const apps = (requests.data?.items ?? []).filter((r) => r.kind === "app" && r.app_id);
  if (apps.length === 0) return null;
  const routes = wsRoutes(params?.org ?? "", params?.ws ?? "");
  return (
    <Notice tone="pending" role="status" aria-label="Requests for access" className="space-y-2" data-testid="requests-banner">
      <ul className="space-y-2">
        {apps.map((r) => {
          const app = r.app_name ?? r.app_id ?? "";
          return (
            <li key={r.id} className="flex flex-wrap items-center gap-x-3 gap-y-1">
              <span className="min-w-0 break-words">
                <strong className="font-semibold">{r.agent_name}</strong> asked for <strong className="font-semibold">{app}</strong>
              </span>
              {canOwn ? (
                <Button asChild size="sm" variant="outline">
                  <Link href={routes.agent(r.agent_id, { tab: "access", connect: r.app_id ?? "" })}>
                    Give {r.agent_name} {app}
                  </Link>
                </Button>
              ) : null}
            </li>
          );
        })}
      </ul>
    </Notice>
  );
}
