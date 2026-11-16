"use client";

import Link from "next/link";
import { useEffect, useState } from "react";

import { Button } from "@/components/ui/button";
import { Notice } from "@/components/ui/notice";
import { useMeOnboarding, usePatchOnboarding } from "@/lib/api/onboarding";
import { useMe, useOrgs, useWorkspaces } from "@/lib/api/queries";
import { MOVED_EVENT, movedRows, RENAMES_NOTICE, type MovedContext } from "@/lib/moved";

/** What the viewer can open, from their organizations and the workspaces of the first one. */
function useMovedContext(): MovedContext {
  const instanceAdmin = useMe().data?.user.is_instance_admin === true;
  const orgs = useOrgs().data?.items ?? [];
  const workspaces = useWorkspaces(orgs[0]?.id).data?.items ?? [];
  const adminOrg = orgs.find((o) => o.my_role === "owner" || o.my_role === "admin")?.slug;
  const first = orgs[0];
  const owned = workspaces.find((w) => w.my_role === "owner");
  const any = workspaces[0];
  return {
    instanceAdmin,
    adminOrg,
    anyOrg: first?.slug,
    ownerWs: first && owned ? { org: first.slug, ws: owned.slug } : undefined,
    anyWs: first && any ? { org: first.slug, ws: any.slug } : undefined,
  };
}

/**
 * "Pages have new names" (C-987): shown once to a person who used the dashboard before, listing old name → new name
 * for the pages they can open. Dismissing stores `renames-2026-11` on the server; **What moved** replays it.
 */
export function RenamesNotice() {
  const onboarding = useMeOnboarding();
  const patch = usePatchOnboarding();
  const context = useMovedContext();
  const [replayed, setReplayed] = useState(false);
  useEffect(() => {
    const show = () => setReplayed(true);
    window.addEventListener(MOVED_EVENT, show);
    return () => window.removeEventListener(MOVED_EVENT, show);
  }, []);

  const seen = onboarding.data?.notices_seen?.includes(RENAMES_NOTICE) ?? true;
  if (!replayed && (onboarding.isPending || seen)) return null;
  const rows = movedRows(context);
  if (rows.length === 0 && !replayed) return null;

  function dismiss() {
    setReplayed(false);
    if (!seen) patch.mutate({ notice_seen: RENAMES_NOTICE });
  }

  return (
    <Notice tone="accent" role="region" aria-labelledby="renames-title" data-testid="renames-notice" className="space-y-2">
      <div className="flex flex-wrap items-start justify-between gap-2">
        <div className="min-w-0">
          <h2 id="renames-title" className="font-display text-base font-semibold">
            Pages have new names
          </h2>
          <p>Every level now has the same pages, with the same names. Nothing was removed.</p>
        </div>
        <Button size="sm" variant="outline" onClick={dismiss}>
          Got it
        </Button>
      </div>
      {rows.length > 0 ? (
        <ul className="grid gap-x-6 gap-y-1 sm:grid-cols-2" aria-label="What moved">
          {rows.map((r) => (
            <li key={`${r.from}→${r.to}`}>
              <Link href={r.href} className="underline underline-offset-2">
                {r.from} → {r.to}
              </Link>
            </li>
          ))}
        </ul>
      ) : null}
    </Notice>
  );
}
