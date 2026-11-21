"use client";

import Link from "next/link";
import { useParams, usePathname, useRouter } from "next/navigation";
import type { ReactNode } from "react";

import { CountBadge } from "@/components/attention/count-badge";
import { ArrowLeftIcon } from "@/components/ui/icons";
import { useOrgBadge } from "@/lib/api/attention";
import { useAltitude } from "@/lib/altitude";
import { visibleTopics } from "@/lib/api/roles";
import { useMe, useOrg, useResolve } from "@/lib/api/queries";
import { pickRailWorkspace, useRailWorkspace } from "@/lib/nav/manage-return";
import { nameInSentence } from "@/lib/workspace-name";
import { TOPIC_LABEL, topicHref, wsRoutes } from "@/lib/routes";
import { cn } from "@/lib/utils";

/** Overview is the organization's own address; every other topic owns the paths below its folder. */
export function manageTopicActive(pathname: string, href: string, topic: string): boolean {
  const path = href.split("?")[0]!;
  return topic === "overview" ? pathname === path : pathname === path || pathname.startsWith(`${path}/`);
}

/**
 * (P5-REFINE-17, rec 8) "Manage <org>": the organization is an area of the page, not a second rail. A **Back to
 * <workspace>** link, the **Manage <org>** header (the only place the organization is named) and a sub-nav (a column on
 * a desktop, a select on a phone) filtered by role with `visibleTopics("org", …)`. Workspace pages (`/o/<org>/w/…`) are
 * not part of it.
 */
export function ManageFrame({ children }: { children: ReactNode }) {
  const { org: slug } = useParams<{ org: string }>();
  const pathname = usePathname();
  const router = useRouter();
  const me = useMe();
  const resolved = useResolve(slug);
  const org = useOrg(resolved.data?.org.id);
  const altitude = useAltitude({ orgId: resolved.data?.org.id });
  const stored = useRailWorkspace();
  // (P5-REFINE-17 #17, C-1082) What waits in this organization: owners and admins only (null for everyone else).
  const badge = useOrgBadge(resolved.data?.org.id);

  if (!slug || /^\/o\/[^/]+\/w\//.test(pathname)) return <>{children}</>;
  // (P5-REFINE-20, org-3) No area, and no name taken from the address, until the organization is known: an unknown one is
  // the shell's not-found state, never a "Manage <slug>" frame.
  if (!resolved.data) return resolved.isError ? <>{children}</> : null;

  const instanceAdmin = me.data?.user.is_instance_admin === true;
  // While the role loads, list what every member sees; the pages refuse nothing they list.
  const topics = visibleTopics("org", { instanceAdmin, org: org.data?.my_role ?? (instanceAdmin ? undefined : "member") }, altitude);
  const orgName = org.data?.name ?? resolved.data?.org.name ?? slug;
  const back = pickRailWorkspace(slug, stored, me.data?.memberships);
  const backHref = back ? wsRoutes(slug, back.wsSlug).home() : "/";
  const items = topics.map((t) => ({ topic: t, href: topicHref("org", t, slug), label: TOPIC_LABEL[t] }));
  const current = items.find((i) => manageTopicActive(pathname, i.href, i.topic));

  return (
    <div data-testid="manage-area" className="space-y-4">
      <div className="space-y-2">
        <Link
          href={backHref}
          className="inline-flex min-h-9 items-center gap-2 rounded-control text-sm text-ink-2 hover:text-ink"
        >
          <ArrowLeftIcon className="size-4 shrink-0 text-muted" />
          Back to {nameInSentence(back?.wsName ?? "Aurora")}
        </Link>
        <p className="font-display text-lg font-semibold text-ink">Manage {orgName}</p>
      </div>
      <div className="md:flex md:items-start md:gap-8">
        <nav aria-label={`Manage ${orgName}`} className="hidden w-48 shrink-0 md:block">
          <ul className="space-y-0.5">
            {items.map((i) => (
              <li key={i.topic}>
                <Link
                  href={i.href}
                  aria-current={current?.topic === i.topic ? "page" : undefined}
                  className={cn(
                    "flex min-h-9 items-center gap-2 rounded-control px-2.5 py-1.5 text-sm hover:bg-subtle",
                    current?.topic === i.topic ? "bg-subtle font-semibold text-ink" : "text-ink-2",
                  )}
                >
                  <span className="min-w-0 flex-1 truncate">{i.label}</span>
                  <CountBadge n={i.topic === "overview" ? badge?.count : badge?.topics[i.topic]} actionId="manage-nav-entry" testId={`manage-attention-badge-${i.topic}`} />
                </Link>
              </li>
            ))}
          </ul>
        </nav>
        <label className="block md:hidden">
          <span className="sr-only">Section of {orgName}</span>
          <select
            aria-label={`Section of ${orgName}`}
            className="h-11 w-full rounded-control border border-line bg-surface px-2.5 text-sm text-ink"
            value={current?.href ?? ""}
            onChange={(e) => router.push(e.target.value)}
          >
            {current ? null : <option value="">Choose a section</option>}
            {items.map((i) => {
              const n = i.topic === "overview" ? badge?.count : badge?.topics[i.topic];
              return (
                <option key={i.topic} value={i.href}>
                  {n ? `${i.label} (${n})` : i.label}
                </option>
              );
            })}
          </select>
        </label>
        <div className="min-w-0 flex-1 pt-4 md:pt-0">{children}</div>
      </div>
    </div>
  );
}
