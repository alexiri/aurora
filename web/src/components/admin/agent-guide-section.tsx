"use client";

import { useState } from "react";

import { Badge } from "@/components/ui/badge";
import { TextViewer } from "@/components/ui/text-viewer";
import { ErrorState, Loading } from "@/components/states";
import { formatBytes } from "@/lib/api/attachments";
import { useAgentGuideFile, type PluginAgentGuide, type PluginPackage } from "@/lib/api/plugins";

type Change = "added" | "changed" | "removed";

const CHANGE_LABEL: Record<Change, string> = { added: "Added", changed: "Changed", removed: "Removed" };

function ChangeBadge({ change }: { change: Change | undefined }) {
  if (!change) return null;
  return <Badge tone={change === "removed" ? "danger" : "pending"}>{CHANGE_LABEL[change]}</Badge>;
}

/** Whether an app's guide has anything an agent could read. */
export function hasGuideContent(g: PluginAgentGuide): boolean {
  return Boolean(g.summary) || g.instructions.trim() !== "" || g.files.length > 0 || g.operations != null;
}

/** Whether the review has an Agent guide section to read (a package with no guide at all has none). */
export function packageHasGuide(plugin: PluginPackage): boolean {
  return (plugin.agent_guides ?? []).some(hasGuideContent);
}

function FileViewer({ pluginId, appId, file }: { pluginId: string; appId: string; file: string }) {
  const q = useAgentGuideFile(pluginId, appId, file);
  if (q.isPending) return <Loading />;
  if (q.isError) return <ErrorState error={q.error} onRetry={() => q.refetch()} />;
  if (q.data.trim() === "") return <p className="text-muted">This file is empty.</p>;
  return <TextViewer label={`File ${file}`} text={q.data} />;
}

interface Row {
  file: string;
  title: string;
  size: number;
  extra?: string;
}

function FileList({
  heading,
  rows,
  changes,
  selected,
  onSelect,
}: {
  heading: string;
  rows: Row[];
  changes: Map<string, Change>;
  selected: string | null;
  onSelect: (file: string) => void;
}) {
  if (rows.length === 0) return null;
  return (
    <div>
      <h5 className="text-xs font-semibold uppercase tracking-label text-muted">{heading}</h5>
      <ul className="mt-1 divide-y divide-line rounded-control border border-line">
        {rows.map((r) => (
          <li key={r.file} className="flex flex-wrap items-center gap-x-3 gap-y-1 px-3 py-2">
            <button
              type="button"
              aria-pressed={selected === r.file}
              className="min-h-11 break-all rounded-control text-left font-mono text-xs font-medium text-link underline sm:min-h-8"
              onClick={() => onSelect(r.file)}
            >
              {r.file}
            </button>
            <span className="min-w-0 flex-1 break-words text-ink-2">{r.title}</span>
            <span className="text-xs text-muted">
              {r.extra ? `${r.extra}, ` : ""}
              {formatBytes(r.size)}
            </span>
            <ChangeBadge change={changes.get(r.file)} />
          </li>
        ))}
      </ul>
    </div>
  );
}

function AppGuide({ plugin, guide }: { plugin: PluginPackage; guide: PluginAgentGuide }) {
  const [file, setFile] = useState<string | null>(null);
  const name = plugin.apps.find((a) => a.app_id === guide.app_id)?.name ?? guide.app_id;
  const changed = guide.changed ?? null;
  const changes = new Map<string, Change>((changed?.files ?? []).map((f) => [f.file, f.change as Change]));
  const removed = (changed?.files ?? []).filter((f) => f.change === "removed");
  const recipes = guide.files.filter((f) => f.kind === "recipe");
  const reference = guide.files.filter((f) => f.kind !== "recipe");
  const ops = guide.operations ?? null;
  const opsRows: Row[] = ops ? [{ file: ops.file, title: "Operations file", size: ops.size, extra: `${ops.count} operations` }] : [];
  const rows = (list: typeof guide.files): Row[] => list.map((f) => ({ file: f.file, title: f.title, size: f.size }));

  return (
    <div className="min-w-0 space-y-3" data-testid={`agent-guide-${guide.app_id}`}>
      <h4 className="font-medium text-ink">
        {name} <span className="font-mono text-xs font-normal text-muted">{guide.app_id}</span>
      </h4>
      {!hasGuideContent(guide) ? (
        <p className="text-muted">This app ships no agent guide.</p>
      ) : (
        <>
          <p>
            <span className="text-xs font-semibold uppercase tracking-label text-muted">Summary</span>{" "}
            {changed?.summary ? <Badge tone="pending">Changed</Badge> : null}
            <br />
            {guide.summary ? <span className="break-words text-ink">{guide.summary}</span> : <span className="text-muted">No summary: the app description is used.</span>}
          </p>
          {guide.instructions.trim() !== "" ? (
            <div>
              <h5 className="text-xs font-semibold uppercase tracking-label text-muted">
                Essentials {changed?.instructions ? <Badge tone="pending">Changed</Badge> : null}
              </h5>
              <TextViewer label={`Essentials of ${name}`} text={guide.instructions} className="mt-1 max-h-48" />
            </div>
          ) : null}
          <FileList heading="Recipes" rows={rows(recipes)} changes={changes} selected={file} onSelect={setFile} />
          <FileList heading="Reference files" rows={rows(reference)} changes={changes} selected={file} onSelect={setFile} />
          <FileList heading="Operations" rows={opsRows} changes={changes} selected={file} onSelect={setFile} />
          {removed.length > 0 ? (
            <div>
              <h5 className="text-xs font-semibold uppercase tracking-label text-muted">Removed in this version</h5>
              <ul className="mt-1">
                {removed.map((f) => (
                  <li key={f.file} className="flex items-center gap-2 py-1">
                    <span className="break-all font-mono text-xs">{f.file}</span> <Badge tone="danger">Removed</Badge>
                  </li>
                ))}
              </ul>
            </div>
          ) : null}
          {file !== null ? <FileViewer key={file} pluginId={plugin.id} appId={guide.app_id} file={file} /> : null}
        </>
      )}
    </div>
  );
}

/**
 * (P5-GUIDE-3, C-648, `24 §9`) What agents that use each app of the package can read. Third-party text, shown
 * as plain text only; a file is fetched when it is selected.
 */
export function AgentGuideSection({ plugin }: { plugin: PluginPackage }) {
  const guides = plugin.agent_guides ?? [];
  if (guides.length === 0) return <p className="text-muted">This package ships no agent guide.</p>;
  return (
    <div className="space-y-5">
      <p className="text-ink">Agents that use this app can read this text. It is written by the publisher.</p>
      {guides.map((g) => (
        <AppGuide key={g.app_id} plugin={plugin} guide={g} />
      ))}
    </div>
  );
}
