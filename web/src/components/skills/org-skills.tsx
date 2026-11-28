"use client";

import Link from "next/link";
import { useParams } from "next/navigation";

import { OrgGate } from "@/components/org/org-gate";
import { TopicPage } from "@/components/topic-page";
import { ScreenChromeContext } from "@/components/ui/list-detail";
import { Notice } from "@/components/ui/notice";
import type { Org } from "@/lib/api/queries";
import { useSkillLibraries } from "@/lib/api/skill-libraries";
import { orgRoutes, type TABS } from "@/lib/routes";

import { useHeaderSlot } from "./header-slot";
import { SkillLibrariesTab } from "./libraries-page";
import { SkillsList } from "./skills-list";

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
  const header = useHeaderSlot();
  return (
    <TopicPage<Tab>
      idBase="org-skills"
      title={`Skills in ${org.name}`}
      tabsName="Skills"
      description={`What every agent in ${org.name} can do, and where it comes from.`}
      // Add skill belongs to the Skills tab: the list puts it here.
      actions={(tab) => (tab === "skills" ? header.slot : null)}
      tabs={[
        {
          id: "skills",
          label: "Skills",
          content: (
            <div>
              <AddLibraryCallToAction slug={slug} orgId={org.id} />
              <ScreenChromeContext.Provider value={header.chrome}>
                <SkillsList page={{ kind: "organization", orgId: org.id, label: org.name }} />
              </ScreenChromeContext.Provider>
            </div>
          ),
        },
        { id: "libraries", label: "Libraries", content: <SkillLibrariesTab slug={slug} orgId={org.id} orgName={org.name} /> },
      ]}
    />
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
