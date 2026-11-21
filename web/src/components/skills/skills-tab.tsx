"use client";

import { EffectiveSkillsView, SkillsEditor } from "./skills-view";

/** Agent tab: the agent's own skills (editable) and the effective set it receives from all three tiers. */
export function SkillsTab({ agentId, orgId, agentName }: { agentId: string; orgId?: string; agentName?: string }) {
  return (
    <div className="space-y-8">
      <section aria-label="Agent skills" className="space-y-3">
        <h3 className="font-display text-base font-semibold text-ink">This agent&apos;s skills</h3>
        <SkillsEditor scope={{ kind: "agent", id: agentId }} orgId={orgId} agentName={agentName} />
      </section>
      <EffectiveSkillsView agentId={agentId} />
    </div>
  );
}
