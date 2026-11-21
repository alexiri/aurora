"use client";

import { useState } from "react";

import { AdminGuard } from "@/components/admin/admin-guard";
import { EmptyState, ErrorState, Loading } from "@/components/states";
import { Button } from "@/components/ui/button";
import { PageHeader } from "@/components/ui/page-header";
import { ApiError } from "@/lib/api/client";
import { useInstanceSkillLibraries } from "@/lib/api/skill-libraries";
import { adminRoutes } from "@/lib/routes";

import { AddLibraryDialog } from "./add-library-dialog";
import { LibrariesTable } from "./libraries-table";

/** Administration › What agents may do › Skill libraries (D-450(b)): the libraries offered to every organization; instance admins. */
export function InstanceLibrariesPage() {
  return (
    <AdminGuard>
      <InstanceLibraries />
    </AdminGuard>
  );
}

function InstanceLibraries() {
  const libs = useInstanceSkillLibraries();
  const [adding, setAdding] = useState(false);
  const disabled = libs.error instanceof ApiError && libs.error.code === "skill_libraries_disabled";
  const items = libs.data?.items ?? [];
  return (
    <div className="space-y-6">
      <PageHeader
        breadcrumb={[{ label: "What agents may do", href: adminRoutes.agentPolicy("defaults") }, { label: "Skill libraries" }]}
        title="Skill libraries"
        description="Libraries here are offered to every organization. People add skills from them with the same permissions as any skill, and each organization approves its own updates."
        actions={disabled ? null : <Button onClick={() => setAdding(true)}>Add a library</Button>}
      />
      {libs.isPending ? <Loading /> : null}
      {disabled ? <EmptyState title="Skill libraries are turned off on this instance" /> : null}
      {libs.isError && !disabled ? <ErrorState error={libs.error} onRetry={() => libs.refetch()} /> : null}
      {libs.data && items.length === 0 ? (
        <EmptyState title="No libraries yet" action={<Button onClick={() => setAdding(true)}>Add a library</Button>}>
          <p className="text-sm text-ink-2">Add a git repository of skills; every organization then sees it and people add the skills they want.</p>
        </EmptyState>
      ) : null}
      {items.length > 0 ? <LibrariesTable scope="instance" orgId={null} rows={items} canManage caption="Skill libraries of this instance" /> : null}
      <AddLibraryDialog orgId={null} open={adding} onOpenChange={setAdding} />
    </div>
  );
}
