"use client";

import { useEffect, useState } from "react";

import { EmptyState, ErrorState, Loading } from "@/components/states";
import { Button } from "@/components/ui/button";
import { PlusIcon } from "@/components/ui/icons";
import { ApiError } from "@/lib/api/client";
import { useSkillLibraries, type SkillLibrary } from "@/lib/api/skill-libraries";
import { useOrgSkills } from "@/lib/api/skills";
import { orgRoutes } from "@/lib/routes";

import { AddLibraryDialog } from "./libraries/add-library-dialog";
import { LibraryCards } from "./libraries/library-cards";
import { activeLibraries, LibraryBrowserDialog } from "./add-from-library";

/**
 * Manage <org> › Skills › Libraries (docs/14, D-450, D-506): the organization's own libraries and the ones shared with
 * every organization, as cards. The page's header and tabs are `OrgSkills`'.
 */
export function SkillLibrariesTab({ slug, orgId, orgName }: { slug: string; orgId: string; orgName: string }) {
  const libs = useSkillLibraries(orgId);
  const orgSkills = useOrgSkills(orgId);
  const [adding, setAdding] = useState(false);
  const [browsing, setBrowsing] = useState<string | null>(null);
  const disabled = libs.error instanceof ApiError && libs.error.code === "skill_libraries_disabled";
  const items: SkillLibrary[] = libs.data?.items ?? [];
  // The API lists the organization's own first, then the instance's (C-1128); never merged with another organization's.
  const own = items.filter((l) => l.scope !== "instance");
  const shared = items.filter((l) => l.scope === "instance");
  const review = orgRoutes(slug).skills();
  const skills = orgSkills.data?.items;
  // A check changes both lists: the skills are read again when a library's waiting updates change, so a card never
  // says an update waits elsewhere only because the skills are the older read.
  const waiting = items.map((l) => `${l.id}:${l.pending_updates ?? 0}`).join();
  const { refetch: readSkills } = orgSkills;
  useEffect(() => {
    if (waiting) void readSkills();
  }, [waiting, readSkills]);
  const groups = activeLibraries(items);
  return (
    <div className="space-y-8">
      {libs.isPending ? <Loading /> : null}
      {disabled ? <EmptyState title="Skill libraries are turned off on this instance" /> : null}
      {libs.isError && !disabled ? <ErrorState error={libs.error} onRetry={() => libs.refetch()} /> : null}
      {libs.data ? (
        <div className="space-y-3">
          <div className="flex flex-wrap items-center gap-x-4 gap-y-2">
            <p className="min-w-0 flex-1 basis-64 text-sm text-ink-2">Libraries are git repositories of skills. Add one, then pick the skills {orgName} should use.</p>
            <Button onClick={() => setAdding(true)}>
              <PlusIcon className="size-4" />
              Add a library
            </Button>
          </div>
          {own.length === 0 ? (
            <EmptyState title="No libraries yet" action={<Button onClick={() => setAdding(true)}>Add a library</Button>}>
              <p className="text-sm text-ink-2">Add a git repository of skills; people then add the skills they want to their agents, workspaces or the organization.</p>
            </EmptyState>
          ) : (
            <LibraryCards
              orgId={orgId}
              orgName={orgName}
              rows={own}
              canManage
              label="Your organization's libraries"
              orgSkills={skills}
              reviewHref={review}
              onBrowse={(l) => setBrowsing(l.id)}
            />
          )}
        </div>
      ) : null}
      {shared.length > 0 ? (
        <section aria-labelledby="shared-libraries" className="space-y-3">
          <h2 id="shared-libraries" className="font-display text-lg font-semibold text-ink">
            Shared with every organization
          </h2>
          <p className="text-sm text-ink-2">Set up by the administrators of this installation. Anyone who adds skills can use them; you approve their updates for your organization.</p>
          <LibraryCards
            orgId={orgId}
            orgName={orgName}
            rows={shared}
            canManage={false}
            label="Libraries shared with every organization"
            orgSkills={skills}
            reviewHref={review}
            onBrowse={(l) => setBrowsing(l.id)}
          />
        </section>
      ) : null}
      <AddLibraryDialog orgId={orgId} open={adding} onOpenChange={setAdding} />
      <LibraryBrowserDialog key={browsing ?? "none"} orgId={orgId} groups={groups} open={browsing !== null} onOpenChange={(o) => !o && setBrowsing(null)} initialId={browsing ?? undefined} />
    </div>
  );
}
