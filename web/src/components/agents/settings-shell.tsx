"use client";

import type { ReactNode } from "react";

import { AgentFreeze } from "@/components/freeze/freeze-sections";
import { Button } from "@/components/ui/button";
import { ChevronLeftIcon } from "@/components/ui/icons";
import { DangerZone } from "@/components/ui/section";
import { Tabs } from "@/components/ui/tabs";
import { authorityRank, useFreezeAgent, useFreezeAuthority, type Frozen } from "@/lib/api/freeze";

import { PAGE_LABEL, SETTINGS_LEAD, type AgentPageId } from "./sections";

/** (D-407) Resume, in the danger zone of a paused agent. Whoever paused it at a higher authority keeps the say. */
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
 * (P5-REFINE-11, D-408) The Settings page: the page header (back link, the page's name and its one-line lead), a sub-nav
 * (a left column from `md`, the segmented tabs below it), the page itself, and at the bottom the danger zone with
 * **Pause <Agent>** and **Delete <Agent>…** for workspace owners. The pages' own bodies are other briefs'; this is the frame.
 */
export type SettingsShellProps = Omit<Parameters<typeof SettingsShell>[0], "children">;

/** Wraps the page bodies in the Settings frame on a Settings page, and renders them as they are anywhere else. */
export function SettingsFrame({ shell, children }: { shell: SettingsShellProps | null; children: ReactNode }) {
  return shell ? <SettingsShell {...shell}>{children}</SettingsShell> : <>{children}</>;
}

export function SettingsShell({
  agentId,
  agentName,
  orgId,
  page,
  pages,
  onPage,
  onBack,
  backLabel,
  canOwn,
  frozen,
  onDelete,
  children,
}: {
  agentId: string;
  agentName: string;
  orgId?: string;
  page: AgentPageId;
  pages: { id: AgentPageId; label: string }[];
  onPage: (p: AgentPageId) => void;
  onBack: () => void;
  backLabel: string;
  canOwn: boolean;
  frozen: Frozen | null | undefined;
  onDelete: () => void;
  children: ReactNode;
}) {
  const own = canOwn && !!orgId;
  const levelHere = frozen ? frozen.level === "agent" : false;
  return (
    <div className="space-y-3">
      <div className="space-y-1">
        <button type="button" onClick={onBack} className="inline-flex min-h-11 items-center gap-1 text-sm text-ink-2 hover:text-ink md:min-h-0">
          <ChevronLeftIcon className="size-4" aria-hidden="true" />
          {backLabel}
        </button>
        <h2 className="font-display text-xl font-semibold text-ink">{PAGE_LABEL[page]}</h2>
        <p className="text-sm text-ink-2">{SETTINGS_LEAD[page]}</p>
      </div>
      <div className="space-y-4 md:flex md:items-start md:gap-6 md:space-y-0">
        {pages.length > 1 ? (
          <Tabs
            variant="segmented"
            items={pages.map((p) => ({ value: p.id, label: p.label }))}
            value={page}
            onValueChange={(v) => onPage(v as AgentPageId)}
            label="Settings pages"
            idBase="agent"
            columnFromMd
            className="max-md:max-w-full md:w-52 md:shrink-0 md:flex-col md:bg-transparent md:p-0"
            tabClassName="md:w-full md:justify-start md:rounded-control md:px-3"
          />
        ) : null}
        {/* The page body brings its own tabpanel wrapper (agent-page.tsx). */}
        <div className="min-w-0 md:flex-1">{children}</div>
      </div>
      {own ? (
        <DangerZone id="agent-danger" title="Pause or delete" className="md:ml-58">
          <ul className="divide-y divide-line">
            <li className="flex flex-wrap items-center justify-between gap-x-4 gap-y-2 py-2 first:pt-0">
              <p className="min-w-0 text-sm text-ink-2">{frozen ? `${agentName} is paused. Nothing runs until you resume it.` : "Stops it until you resume it"}</p>
              {frozen ? (
                levelHere ? <ResumeButton agentId={agentId} name={agentName} orgId={orgId!} frozen={frozen} /> : null
              ) : (
                <AgentFreeze agentId={agentId} name={agentName} orgId={orgId!} frozen={frozen} canOwn buttonLabel={`Pause ${agentName}`} buttonVariant="outline" />
              )}
            </li>
            <li className="flex flex-wrap items-center justify-between gap-x-4 gap-y-2 py-2 last:pb-0">
              <p className="min-w-0 text-sm text-ink-2">Removes the agent and revokes all of its credentials. This cannot be undone.</p>
              <Button type="button" variant="danger" size="sm" onClick={onDelete}>
                Delete {agentName}…
              </Button>
            </li>
          </ul>
        </DangerZone>
      ) : null}
    </div>
  );
}
