"use client";

import type { ReactNode } from "react";

import { EmbeddedPage, PageHeader, type PageLevel } from "@/components/ui/page-header";
import { Tabs, tabId, tabPanelId } from "@/components/ui/tabs";
import { useTabParam } from "@/lib/use-tab-param";

export interface TopicTab<T extends string = string> {
  id: T;
  label: string;
  /** Rendered only when this tab is selected. */
  content: ReactNode;
}

/** Query parameters that belong to one tab's dialogs and filters: switching tabs drops them. */
const TAB_SCOPED_PARAMS = ["connect", "reconnect", "kind", "package", "client_key", "add"];

/**
 * One topic page (`14 §Topics per level`): the header with its level badge, at most four tabs addressed by
 * `?tab=`, and the selected tab's content. A tab hosts the content of the page it replaces, whose own header
 * keeps only its description and actions (`EmbeddedPage`).
 */
export function TopicPage<T extends string>({
  title,
  description,
  level,
  tabs,
  idBase,
  actions,
  width = "max-w-page",
  children,
}: {
  title: string;
  description?: ReactNode;
  level?: PageLevel;
  tabs: readonly TopicTab<T>[];
  idBase: string;
  actions?: ReactNode;
  width?: string;
  /** Content above the tab bar's panel, for a page without tabs. */
  children?: ReactNode;
}) {
  const ids = tabs.map((t) => t.id);
  const [tab, setTab] = useTabParam<T>(ids, ids[0]!, { drop: () => TAB_SCOPED_PARAMS });
  const current = tabs.find((t) => t.id === tab) ?? tabs[0];
  return (
    <div className={`${width} space-y-4`}>
      <PageHeader title={title} description={description} level={level} actions={actions} className="mb-2" />
      {tabs.length > 1 ? (
        <Tabs
          items={tabs.map((t) => ({ value: t.id, label: t.label }))}
          value={current?.id ?? ""}
          onValueChange={(v) => setTab(v as T)}
          variant="underline"
          label={`${title} sections`}
          idBase={idBase}
        />
      ) : null}
      {children}
      {current ? (
        <div
          role={tabs.length > 1 ? "tabpanel" : undefined}
          id={tabs.length > 1 ? tabPanelId(idBase, current.id) : undefined}
          aria-labelledby={tabs.length > 1 ? tabId(idBase, current.id) : undefined}
          className="space-y-4 pt-2"
        >
          <EmbeddedPage>{current.content}</EmbeddedPage>
        </div>
      ) : null}
    </div>
  );
}
