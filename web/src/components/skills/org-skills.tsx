"use client";

import Link from "next/link";
import { useParams } from "next/navigation";
import { useState } from "react";

import { OrgGate } from "@/components/org/org-gate";
import { TopicPage } from "@/components/topic-page";
import { Notice } from "@/components/ui/notice";
import type { Org } from "@/lib/api/queries";
import { useSkillLibraries } from "@/lib/api/skill-libraries";
import { orgRoutes, type TABS } from "@/lib/routes";

import { AddSkillButton } from "./add-skill-button";
import { SkillLibrariesTab } from "./libraries-page";
import { SkillsEditor } from "./skills-view";

type Tab = (typeof TABS)["orgSkills"][number];

/** (D-506(a)) Manage <org> › Skills, with the tabs Skills · Libraries; owners and admins. */
export function OrgSkills() {
  const { org: slug } = useParams<{ org: string }>();
  return (
    <OrgGate slug={slug} allow={["owner", "admin"]}>
      {(org) => <OrgSkillsPage slug={slug} org={org} />}
    </OrgGate>
  );
}

function OrgSkillsPage({ slug, org }: { slug: string; org: Org }) {
  const [creating, setCreating] = useState(false);
  return (
    <TopicPage<Tab>
      idBase="org-skills"
      title="Skills"
      description={`What every agent in ${org.name} can do, and where it comes from.`}
      actions={(tab) => (tab === "skills" ? <AddSkillButton onClick={() => setCreating(true)} /> : null)}
      tabs={[
        { id: "skills", label: "Skills", content: <OrgSkillsTab slug={slug} orgId={org.id} create={{ open: creating, onOpenChange: setCreating }} /> },
        { id: "libraries", label: "Libraries", content: <SkillLibrariesTab slug={slug} orgId={org.id} orgName={org.name} /> },
      ]}
    />
  );
}

function OrgSkillsTab({ slug, orgId, create }: { slug: string; orgId: string; create: { open: boolean; onOpenChange: (open: boolean) => void } }) {
  return (
    <div>
      <AddLibraryCallToAction slug={slug} orgId={orgId} />
      <SkillsEditor scope={{ kind: "organization", id: orgId }} orgId={orgId} create={create} />
      {/* (P5-REFINE-20, org-12) The precedence lives under "How skills combine", as on the workspace's page. */}
      <details className="mt-6 rounded-card border border-line bg-surface px-5 py-3 text-sm" data-testid="skills-combine">
        <summary className="cursor-pointer font-semibold text-ink">How skills combine</summary>
        <p className="mt-2 text-ink-2">
          A workspace skill or an agent&apos;s own skill with the same name overrides an organization skill for that workspace or agent. Workspace owners manage
          workspace skills on each workspace&apos;s Skills page.
        </p>
      </details>
    </div>
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
        <Link href={orgRoutes(slug).skills("libraries")} className="text-link underline">
          Add a library
        </Link>
        .
      </Notice>
    </div>
  );
}
