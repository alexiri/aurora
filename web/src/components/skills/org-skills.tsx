"use client";

import { useParams } from "next/navigation";

import { OrgGate } from "@/components/org/org-gate";
import { PageHeader } from "@/components/ui/page-header";

import { SkillsEditor } from "./skills-view";

export function OrgSkills() {
  const { org: slug } = useParams<{ org: string }>();
  return (
    <OrgGate slug={slug} allow={["owner", "admin"]}>
      {(org) => (
        <div className="max-w-[1160px]">
          <PageHeader title="Organization skills" description="Skills every agent in this organization can use." />
          <SkillsEditor scope={{ kind: "organization", id: org.id }} />
        </div>
      )}
    </OrgGate>
  );
}
