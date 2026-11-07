"use client";

import { OrgSkillsReadOnly, SkillsEditor } from "./skills-view";

export function SkillsTab({ agentId, orgId }: { agentId: string; orgId: string }) {
  return (
    <div className="space-y-8">
      <SkillsEditor scope={{ kind: "agent", id: agentId }} />
      <OrgSkillsReadOnly orgId={orgId} />
    </div>
  );
}
