"use client";

import { useState } from "react";

import { Badge } from "@/components/ui/badge";
import { TextViewer } from "@/components/ui/text-viewer";
import { ErrorState, Loading } from "@/components/states";
import { formatBytes } from "@/lib/api/attachments";
import { useAgentFile, type AgentFileRef, type PluginAgent, type PluginPackage } from "@/lib/api/plugins";

type Change = "added" | "changed" | "removed" | "mode";

const CHANGE_LABEL: Record<Change, string> = { added: "Added", changed: "Changed", removed: "Removed", mode: "Script bit" };

function ChangeBadge({ change }: { change: Change | undefined }) {
  if (!change) return null;
  return <Badge tone={change === "removed" ? "danger" : change === "added" ? "accent" : "neutral"}>{CHANGE_LABEL[change]}</Badge>;
}

/** Whether an app has anything an agent could read. */
export function hasGuideContent(g: PluginAgent): boolean {
  return Boolean(g.summary) || g.instructions.trim() !== "" || g.skills.length > 0 || g.operations != null;
}

/** Whether the review has an agent section to read (a package that gives agents nothing has none). */
export function packageHasGuide(plugin: PluginPackage): boolean {
  return (plugin.agent ?? []).some(hasGuideContent);
}

function FileViewer({ pluginId, appId, file, label }: { pluginId: string; appId: string; file: AgentFileRef; label: string }) {
  const q = useAgentFile(pluginId, appId, file);
  if (q.isPending) return <Loading />;
  if (q.isError) return <ErrorState error={q.error} onRetry={() => q.refetch()} />;
  if (q.data.trim() === "") return <p className="text-muted">This file is empty.</p>;
  return <TextViewer label={`File ${label}`} text={q.data} />;
}

interface Row {
  /** Unique within the app; also the button text. */
  id: string;
  ref: AgentFileRef;
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
  onSelect: (row: Row) => void;
}) {
  if (rows.length === 0) return null;
  return (
    <div>
      <h5 className="text-xs font-semibold uppercase tracking-label text-muted">{heading}</h5>
      <ul className="mt-1 divide-y divide-line rounded-control border border-line">
        {rows.map((r) => (
          <li key={r.id} className="flex flex-wrap items-center gap-x-3 gap-y-1 px-3 py-2">
            <button
              type="button"
              aria-pressed={selected === r.id}
              className="min-h-11 break-all rounded-control text-left font-mono text-xs font-medium text-link underline sm:min-h-8"
              onClick={() => onSelect(r)}
            >
              {r.id}
            </button>
            <span className="min-w-0 flex-1 break-words text-ink-2">{r.title}</span>
            <span className="text-xs text-muted">
              {r.extra ? `${r.extra}, ` : ""}
              {formatBytes(r.size)}
            </span>
            <ChangeBadge change={changes.get(r.id)} />
          </li>
        ))}
      </ul>
    </div>
  );
}

function AppAgent({ plugin, agent }: { plugin: PluginPackage; agent: PluginAgent }) {
  const [sel, setSel] = useState<Row | null>(null);
  const name = plugin.apps.find((a) => a.app_id === agent.app_id)?.name ?? agent.app_id;
  const changed = agent.changed ?? null;
  const changes = new Map<string, Change>();
  const removed: string[] = [];
  for (const s of changed?.skills ?? []) {
    for (const f of s.files) {
      changes.set(`${s.name}/${f.path}`, f.change as Change);
      if (f.change === "removed") removed.push(`${s.name}/${f.path}`);
    }
  }
  const ops = agent.operations ?? null;
  const opsRows: Row[] = ops
    ? [{ id: ops.file, ref: { kind: "operations" }, title: "Operations file", size: ops.size, extra: `${ops.count} operations` }]
    : [];
  const skillRows: Row[] = agent.skills.flatMap((s) =>
    s.files.map((f) => ({
      id: `${s.name}/${f.path}`,
      ref: { kind: "skill", name: s.name, path: f.path === "SKILL.md" ? null : f.path } as AgentFileRef,
      title: f.path === "SKILL.md" ? s.description : "",
      size: f.size,
      extra: f.executable ? "script" : undefined,
    })),
  );

  return (
    <div className="min-w-0 space-y-3" data-testid={`agent-guide-${agent.app_id}`}>
      <h4 className="font-medium text-ink">
        {name} <span className="font-mono text-xs font-normal text-muted">{agent.app_id}</span>
      </h4>
      {!hasGuideContent(agent) ? (
        <p className="text-muted">This app gives agents nothing but its description.</p>
      ) : (
        <>
          <p>
            <span className="text-xs font-semibold uppercase tracking-label text-muted">Summary</span>{" "}
            {changed?.summary ? <Badge>Changed</Badge> : null}
            <br />
            {agent.summary ? <span className="break-words text-ink">{agent.summary}</span> : <span className="text-muted">No summary: the app description is used.</span>}
          </p>
          {agent.instructions.trim() !== "" ? (
            <div>
              <h5 className="text-xs font-semibold uppercase tracking-label text-muted">
                Essentials {changed?.instructions ? <Badge>Changed</Badge> : null}
              </h5>
              <TextViewer label={`Essentials of ${name}`} text={agent.instructions} className="mt-1 max-h-48" />
            </div>
          ) : null}
          <FileList heading="Skills" rows={skillRows} changes={changes} selected={sel?.id ?? null} onSelect={setSel} />
          <FileList heading="Operations" rows={opsRows} changes={changes} selected={sel?.id ?? null} onSelect={setSel} />
          {removed.length > 0 ? (
            <div>
              <h5 className="text-xs font-semibold uppercase tracking-label text-muted">Removed in this version</h5>
              <ul className="mt-1">
                {removed.map((f) => (
                  <li key={f} className="flex items-center gap-2 py-1">
                    <span className="break-all font-mono text-xs">{f}</span> <Badge tone="danger">Removed</Badge>
                  </li>
                ))}
              </ul>
            </div>
          ) : null}
          {sel !== null ? <FileViewer key={sel.id} pluginId={plugin.id} appId={agent.app_id} file={sel.ref} label={sel.id} /> : null}
        </>
      )}
    </div>
  );
}

/**
 * (C-1136, `24 §9`) What agents that use each app of the package get: the summary, the essentials, the skills
 * (scripts marked) and the operations file. Third-party text, shown as plain text only; a file is fetched when it
 * is selected. P5-APPSKILL-3 gives this section its final form.
 */
export function AgentGuideSection({ plugin }: { plugin: PluginPackage }) {
  const agents = plugin.agent ?? [];
  if (agents.length === 0) return <p className="text-muted">This package gives agents nothing but its description.</p>;
  return (
    <div className="space-y-5">
      <p className="text-ink">Agents that use this app get these skills. They are written by the publisher; scripts run in the agent&apos;s sandbox with the agent&apos;s access.</p>
      {agents.map((g) => (
        <AppAgent key={g.app_id} plugin={plugin} agent={g} />
      ))}
    </div>
  );
}
