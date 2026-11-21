"use client";

import Link from "next/link";
import { useParams } from "next/navigation";

import { OrgGate } from "@/components/org/org-gate";
import { Button } from "@/components/ui/button";
import { Notice } from "@/components/ui/notice";
import { PageHeader } from "@/components/ui/page-header";

import { SkillsEditor } from "./skills-view";
import { useSkillLibraries } from "@/lib/api/skill-libraries";
import { orgRoutes } from "@/lib/routes";

export function OrgSkills() {
  const { org: slug } = useParams<{ org: string }>();
  return (
    <OrgGate slug={slug} allow={["owner", "admin"]}>
      {(org) => (
        <div>
          <PageHeader
            title="Organization skills"
            description="Skills every agent in this organization can use."
            actions={<LibrariesLink slug={slug} orgId={org.id} />}
          />
          <AddLibraryCallToAction slug={slug} orgId={org.id} />
          <SkillsEditor scope={{ kind: "organization", id: org.id }} orgId={org.id} />
          {/* (P5-REFINE-20, org-12) The precedence lives under "How skills combine", as on the workspace's page. */}
          <details className="mt-6 rounded-card border border-line bg-surface px-5 py-3 text-sm" data-testid="skills-combine">
            <summary className="cursor-pointer font-semibold text-ink">How skills combine</summary>
            <p className="mt-2 text-ink-2">
              A workspace skill or an agent&apos;s own skill with the same name overrides an organization skill for that workspace or agent. Workspace owners manage
              workspace skills on each workspace&apos;s Skills page.
            </p>
          </details>
        </div>
      )}
    </OrgGate>
  );
}

function LibrariesLink({ slug, orgId }: { slug: string; orgId: string }) {
  const libs = useSkillLibraries(orgId);
  // Hidden while the feature is switched off on this instance (404 skill_libraries_disabled).
  if (libs.isError) return null;
  return (
    <Button asChild variant="outline">
      <Link href={orgRoutes(slug).skillLibraries()}>Add a library</Link>
    </Button>
  );
}

/** With no library connected, owners and admins see an inline call to action (docs/14). */
function AddLibraryCallToAction({ slug, orgId }: { slug: string; orgId: string }) {
  const libs = useSkillLibraries(orgId);
  if (!libs.data || libs.data.items.length > 0) return null;
  return (
    <div className="mb-4">
      <Notice role="status">
        Skills can also come from a git repository.{" "}
        <Link href={orgRoutes(slug).skillLibraries()} className="text-link underline">
          Add a library
        </Link>
        .
      </Notice>
    </div>
  );
}
