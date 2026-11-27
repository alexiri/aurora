"use client";

import { useMemo, useState, type ReactNode } from "react";

import { ScreenChromeContext, type ScreenChrome } from "@/components/ui/list-detail";
import { SubTabs } from "@/components/ui/sub-tabs";
import { cn } from "@/lib/utils";

import { PAGE_LABEL, SETTINGS_LEAD, type AgentPageId } from "./sections";

/**
 * A Settings page's list and item are two screens below this width (D-507). With the Settings column gone the content
 * is the window less the rail and the page padding, so from 1024 px the list and an item of at least 24 rem fit.
 */
export const SETTINGS_NARROW = "(max-width: 1023px)";

/**
 * (D-507) A page of the agent's Settings tab, under the agent's header and tab row: the second row (the Settings
 * sections), the page's title row (its name, one-line lead and its main action) and the page itself, at the full content
 * width. Pause, Export/Import and Delete are the owners' last page, Manage (D-447, `manage-page.tsx`), not part of the
 * frame. The pages' own bodies are other briefs'.
 */
export type SettingsShellProps = Omit<Parameters<typeof SettingsShell>[0], "children">;

/** Wraps the page bodies in the Settings frame on a Settings page, and renders them as they are anywhere else. */
export function SettingsFrame({ shell, children }: { shell: SettingsShellProps | null; children: ReactNode }) {
  return shell ? <SettingsShell {...shell}>{children}</SettingsShell> : <>{children}</>;
}

export function SettingsShell({
  agentName,
  page,
  pages,
  onPage,
  children,
}: {
  agentName: string;
  page: AgentPageId;
  pages: { id: AgentPageId; label: string }[];
  onPage: (p: AgentPageId) => void;
  children: ReactNode;
}) {
  // (P5-MEMORY-2) A page may put its main action beside the title, and hide the second row and the title while one of
  // its items has its own screen (`ui/list-detail`).
  const [actionSlot, setActionSlot] = useState<HTMLDivElement | null>(null);
  const [subScreen, setSubScreen] = useState(false);
  const chrome = useMemo<ScreenChrome>(() => ({ actionSlot, setSubScreen, narrowQuery: SETTINGS_NARROW }), [actionSlot]);
  return (
    <div className="space-y-5">
      {pages.length > 1 ? (
        <SubTabs
          items={pages.map((p) => ({ value: p.id, label: p.label }))}
          value={page}
          onValueChange={(v) => onPage(v as AgentPageId)}
          label="Settings pages"
          idBase="agent"
          className={cn(subScreen && "hidden")}
        />
      ) : null}
      <div className={cn("flex items-end justify-between gap-x-4", subScreen && "hidden")}>
        <div className="min-w-0">
          <h2 className="font-display text-2xl font-semibold text-ink">{PAGE_LABEL[page]}</h2>
          <p className="text-sm text-ink-2">{page === "manage" ? `${SETTINGS_LEAD.manage} ${agentName}` : SETTINGS_LEAD[page]}</p>
        </div>
        <div ref={setActionSlot} className="shrink-0 empty:hidden" />
      </div>
      {/* The page body brings its own tabpanel wrapper (agent-page.tsx). */}
      <div className="min-w-0">
        <ScreenChromeContext.Provider value={chrome}>{children}</ScreenChromeContext.Provider>
      </div>
    </div>
  );
}
