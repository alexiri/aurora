"use client";

import { AgentExportImport } from "@/components/agents/export-import";
import { AgentFreeze } from "@/components/freeze/freeze-sections";
import { Button } from "@/components/ui/button";
import { DangerZone, Section } from "@/components/ui/section";
import { authorityRank, useFreezeAgent, useFreezeAuthority, type Frozen } from "@/lib/api/freeze";
import type { Agent } from "@/lib/api/queries";

/** (D-407) Resume, for a paused agent. Whoever paused it at a higher authority keeps the say. */
function ResumeButton({ agentId, name, orgId, frozen }: { agentId: string; name: string; orgId: string; frozen: Frozen }) {
  const m = useFreezeAgent(agentId);
  const rank = useFreezeAuthority(orgId);
  const tooLow = authorityRank(frozen.authority) > rank;
  return (
    <Button type="button" variant="outline" size="sm" disabled={m.unfreeze.isPending || tooLow} onClick={() => void m.unfreeze.mutateAsync().catch(() => undefined)}>
      {m.unfreeze.isPending ? "Resuming…" : `Resume ${name}`}
    </Button>
  );
}

/**
 * (D-447, FIX-213) The last Settings page, for workspace owners: **Pause** (or the paused state with **Resume**), **Export and
 * import** (Expert, hosted agents only) and **Delete** in the danger style. These sat under every Settings page before.
 */
export function ManagePage({
  agent,
  orgId,
  byo,
  onDelete,
}: {
  agent: Agent;
  orgId: string;
  byo: boolean;
  onDelete: () => void;
}) {
  const frozen = agent.frozen;
  const levelHere = frozen ? frozen.level === "agent" : false;
  return (
    <div className="space-y-4">
      <Section id="agent-pause" title="Pause" card>
        <div className="flex flex-wrap items-center justify-between gap-x-4 gap-y-2">
          <p className="min-w-0 text-sm text-ink-2">{frozen ? `${agent.name} is paused. Nothing runs until you resume it.` : "Stops it until you resume it"}</p>
          {frozen ? (
            levelHere ? <ResumeButton agentId={agent.id} name={agent.name} orgId={orgId} frozen={frozen} /> : null
          ) : (
            <AgentFreeze agentId={agent.id} name={agent.name} orgId={orgId} frozen={frozen} canOwn buttonLabel={`Pause ${agent.name}`} buttonVariant="outline" />
          )}
        </div>
      </Section>
      {byo ? null : <AgentExportImport agent={agent} />}
      <DangerZone id="agent-danger" title="Delete">
        <div className="flex flex-wrap items-center justify-between gap-x-4 gap-y-2">
          <p className="min-w-0 text-sm text-ink-2">Removes the agent and revokes all of its credentials. This cannot be undone.</p>
          <Button type="button" variant="danger" size="sm" onClick={onDelete}>
            Delete {agent.name}…
          </Button>
        </div>
      </DangerZone>
    </div>
  );
}
