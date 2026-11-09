"use client";

import { useParams } from "next/navigation";

import { OrgGate } from "@/components/org/org-gate";
import { Notice } from "@/components/ui/notice";
import { PageHeader } from "@/components/ui/page-header";

import { SkillsEditor } from "./skills-view";

export function OrgSkills() {
  const { org: slug } = useParams<{ org: string }>();
  return (
    <OrgGate slug={slug} allow={["owner", "admin"]}>
      {(org) => (
        <div className="max-w-[1160px]">
          <PageHeader title="Organization skills" description="Skills every agent in this organization can use." />
          <div className="mb-4">
            <Notice role="status">
              A workspace skill or an agent&apos;s own skill with the same name overrides an organization skill for that workspace or agent. Workspace owners manage
              workspace skills on each workspace&apos;s Skills page.
            </Notice>
          </div>
          <SkillsEditor scope={{ kind: "organization", id: org.id }} />
        </div>
      )}
    </OrgGate>
  );
}
