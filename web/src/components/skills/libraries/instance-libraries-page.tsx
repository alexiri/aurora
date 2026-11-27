"use client";

import { useState } from "react";

import { EmptyState, ErrorState, Loading } from "@/components/states";
import { Button } from "@/components/ui/button";
import { PlusIcon } from "@/components/ui/icons";
import { ApiError } from "@/lib/api/client";
import { useInstanceSkillLibraries } from "@/lib/api/skill-libraries";

import { AddLibraryDialog } from "./add-library-dialog";
import { LibraryCards } from "./library-cards";

/** Administration › Skills (D-450(b), D-506(c)): the libraries offered to every organization. The page and its guard are `AdminSkillsPage`'s. */
export function InstanceLibraries() {
  const libs = useInstanceSkillLibraries();
  const [adding, setAdding] = useState(false);
  const disabled = libs.error instanceof ApiError && libs.error.code === "skill_libraries_disabled";
  const items = libs.data?.items ?? [];
  return (
    <section aria-labelledby="instance-libraries" className="space-y-3">
      <h2 id="instance-libraries" className="font-display text-lg font-semibold text-ink">
        Libraries
      </h2>
      <div className="flex flex-wrap items-center gap-x-4 gap-y-2">
        <p className="min-w-0 flex-1 basis-64 text-sm text-ink-2">
          Libraries here are offered to every organization. People add skills from them with the same permissions as any skill, and each organization approves its own updates.
        </p>
        {disabled ? null : (
          <Button onClick={() => setAdding(true)}>
            <PlusIcon className="size-4" />
            Add a library
          </Button>
        )}
      </div>
      {libs.isPending ? <Loading /> : null}
      {disabled ? <EmptyState title="Skill libraries are turned off on this instance" /> : null}
      {libs.isError && !disabled ? <ErrorState error={libs.error} onRetry={() => libs.refetch()} /> : null}
      {libs.data && items.length === 0 ? (
        <EmptyState title="No libraries yet" action={<Button onClick={() => setAdding(true)}>Add a library</Button>}>
          <p className="text-sm text-ink-2">Add a git repository of skills; every organization then sees it and people add the skills they want.</p>
        </EmptyState>
      ) : null}
      {items.length > 0 ? <LibraryCards orgId={null} rows={items} canManage label="Skill libraries of this instance" /> : null}
      <AddLibraryDialog orgId={null} open={adding} onOpenChange={setAdding} />
    </section>
  );
}
