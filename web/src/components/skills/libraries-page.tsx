"use client";

import Link from "next/link";
import { useParams } from "next/navigation";
import { useState, type ReactNode } from "react";

import { OrgGate } from "@/components/org/org-gate";
import { EmptyState, ErrorState, Loading } from "@/components/states";
import { Button } from "@/components/ui/button";
import { PageHeader } from "@/components/ui/page-header";
import { ApiError } from "@/lib/api/client";
import { useSkillLibraries, type SkillLibrary } from "@/lib/api/skill-libraries";
import { orgRoutes } from "@/lib/routes";

import { AddLibraryDialog } from "./libraries/add-library-dialog";
import { LibrariesTable } from "./libraries/libraries-table";
import { activeLibraries, LibraryBrowserDialog } from "./library";

/** Skills → Libraries (docs/14, D-450): the organization's own libraries and the ones shared with every organization; org owners and admins. */
export function SkillLibrariesPage() {
  const { org: slug } = useParams<{ org: string }>();
  return (
    <OrgGate slug={slug} allow={["owner", "admin"]}>
      {(org) => <Libraries slug={slug} orgId={org.id} />}
    </OrgGate>
  );
}

function SectionHeading({ id, children }: { id: string; children: ReactNode }) {
  return (
    <h2 id={id} className="mb-3 font-display text-lg font-semibold text-ink">
      {children}
    </h2>
  );
}

function Libraries({ slug, orgId }: { slug: string; orgId: string }) {
  const libs = useSkillLibraries(orgId);
  const [adding, setAdding] = useState(false);
  const [browsing, setBrowsing] = useState<string | null>(null);
  const disabled = libs.error instanceof ApiError && libs.error.code === "skill_libraries_disabled";
  const items: SkillLibrary[] = libs.data?.items ?? [];
  // The API lists the organization's own first, then the instance's (C-1128); never merged with another organization's.
  const own = items.filter((l) => l.scope !== "instance");
  const shared = items.filter((l) => l.scope === "instance");
  const review = orgRoutes(slug).agents("skills");
  return (
    <div className="space-y-8">
      <PageHeader
        title="Skill libraries"
        description="Add any git repository as a source of skills. People then add the skills they want to an agent, a workspace or the organization; each is pinned to a commit and read-only."
        actions={
          <>
            <Button asChild variant="outline">
              <Link href={review}>Organization skills</Link>
            </Button>
            {disabled ? null : <Button onClick={() => setAdding(true)}>Add a library</Button>}
          </>
        }
      />
      {libs.isPending ? <Loading /> : null}
      {disabled ? <EmptyState title="Skill libraries are turned off on this instance" /> : null}
      {libs.isError && !disabled ? <ErrorState error={libs.error} onRetry={() => libs.refetch()} /> : null}
      {libs.data ? (
        <section aria-labelledby="own-libraries">
          <SectionHeading id="own-libraries">Your organization&apos;s libraries</SectionHeading>
          {own.length === 0 ? (
            <EmptyState title="No libraries yet" action={<Button onClick={() => setAdding(true)}>Add a library</Button>}>
              <p className="text-sm text-ink-2">Add a git repository of skills; people then add the skills they want to their agents, workspaces or the organization.</p>
            </EmptyState>
          ) : (
            <LibrariesTable scope="org" orgId={orgId} rows={own} canManage caption="Your organization's libraries" reviewHref={review} />
          )}
        </section>
      ) : null}
      {shared.length > 0 ? (
        <section aria-labelledby="shared-libraries">
          <SectionHeading id="shared-libraries">Shared with every organization</SectionHeading>
          <p className="-mt-2 mb-3 text-sm text-ink-2">Set up by the administrators of this installation. Anyone who adds skills can use them; you approve their updates for your organization.</p>
          <LibrariesTable scope="org" orgId={orgId} rows={shared} canManage={false} caption="Libraries shared with every organization" reviewHref={review} onBrowse={(l) => setBrowsing(l.id)} />
        </section>
      ) : null}
      <AddLibraryDialog orgId={orgId} open={adding} onOpenChange={setAdding} />
      <LibraryBrowserDialog key={browsing ?? "none"} orgId={orgId} groups={{ own: [], shared: activeLibraries(shared).shared }} open={browsing !== null} onOpenChange={(o) => !o && setBrowsing(null)} initialId={browsing ?? undefined} />
    </div>
  );
}
