"use client";

import { useMemo, useState, type ReactNode } from "react";

import { ChevronLeftIcon } from "@/components/ui/icons";
import { ScreenChromeContext, type ScreenChrome } from "@/components/ui/list-detail";
import { Tabs } from "@/components/ui/tabs";
import { cn } from "@/lib/utils";

import { PAGE_LABEL, SETTINGS_LEAD, type AgentPageId } from "./sections";

/**
 * (P5-REFINE-11, D-408) The Settings page: the page header (back link, the page's name and its one-line lead), a sub-nav
 * (a left column from `md`, the segmented tabs below it) and the page itself. Pause, Export/Import and Delete are the
 * owners' last page, Manage (D-447, `manage-page.tsx`), not part of the frame. The pages' own bodies are other briefs'.
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
  onBack,
  backLabel,
  children,
}: {
  agentName: string;
  page: AgentPageId;
  pages: { id: AgentPageId; label: string }[];
  onPage: (p: AgentPageId) => void;
  onBack: () => void;
  backLabel: string;
  children: ReactNode;
}) {
  // (P5-MEMORY-2) A page may put its main action beside the heading, and hide the heading and sub-nav while one of its
  // items has its own screen (`ui/list-detail`).
  const [actionSlot, setActionSlot] = useState<HTMLDivElement | null>(null);
  const [subScreen, setSubScreen] = useState(false);
  const chrome = useMemo<ScreenChrome>(() => ({ actionSlot, setSubScreen }), [actionSlot]);
  return (
    <div className="space-y-3">
      <div className={cn("flex flex-wrap items-end justify-between gap-x-4 gap-y-2", subScreen && "hidden")}>
        <div className="min-w-0 space-y-1">
          <button type="button" onClick={onBack} className="inline-flex min-h-11 items-center gap-1 text-sm text-ink-2 hover:text-ink md:min-h-0">
            <ChevronLeftIcon className="size-4" aria-hidden="true" />
            {backLabel}
          </button>
          <h2 className="font-display text-xl font-semibold text-ink">{PAGE_LABEL[page]}</h2>
          <p className="text-sm text-ink-2">{page === "manage" ? `${SETTINGS_LEAD.manage} ${agentName}` : SETTINGS_LEAD[page]}</p>
        </div>
        <div ref={setActionSlot} className="shrink-0 empty:hidden" />
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
            className={cn("max-md:max-w-full max-md:flex-wrap md:w-52 md:shrink-0 md:flex-col md:bg-transparent md:p-0", subScreen && "hidden")}
            tabClassName="md:w-full md:justify-start md:rounded-control md:px-3"
          />
        ) : null}
        {/* The page body brings its own tabpanel wrapper (agent-page.tsx). */}
        <div className="min-w-0 md:flex-1">
          <ScreenChromeContext.Provider value={chrome}>{children}</ScreenChromeContext.Provider>
        </div>
      </div>
    </div>
  );
}
