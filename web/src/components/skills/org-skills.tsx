"use client";

import { useParams } from "next/navigation";

import { OrgGate } from "@/components/org/org-gate";

import { SkillsEditor } from "./skills-view";

export function OrgSkills() {
  const { org: slug } = useParams<{ org: string }>();
  return (
    <OrgGate slug={slug} allow={["owner", "admin"]}>
      {(org) => (
        <div className="max-w-4xl space-y-4">
          <h1 className="text-xl font-semibold">Organization skills</h1>
          <SkillsEditor scope={{ kind: "organization", id: org.id }} />
        </div>
      )}
    </OrgGate>
  );
}
