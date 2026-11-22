"use client";

import { useState } from "react";

import { Badge } from "@/components/ui/badge";
import { Fold } from "@/components/ui/fold";
import { TextViewer } from "@/components/ui/text-viewer";
import { ErrorState, Loading } from "@/components/states";
import { formatBytes } from "@/lib/api/attachments";
import { useAgentFile, type AgentFileRef, type PluginAgent, type PluginPackage } from "@/lib/api/plugins";

type Change = "added" | "changed" | "removed" | "mode";

/** Anchor of the section: the approve line links to it. */
export const AGENT_CONTENT_ID = "for-agents";

function changeBadge(change: Change, nowScript: boolean) {
  if (change === "mode") return <Badge key="mode">{nowScript ? "Now a script" : "No longer a script"}</Badge>;
  const label = { added: "Added", changed: "Changed", removed: "Removed" }[change];
  return (
    <Badge key={change} tone={change === "removed" ? "danger" : change === "added" ? "accent" : "neutral"}>
      {label}
    </Badge>
  );
}

/** Whether an app has anything an agent could read. */
export function hasAgentContent(g: PluginAgent): boolean {
  return Boolean(g.summary) || g.instructions.trim() !== "" || g.skills.length > 0 || g.operations != null;
}

/** Whether the review has a "For agents" section (a package that gives agents nothing has none). */
export function packageHasAgentContent(plugin: PluginPackage): boolean {
  return (plugin.agent ?? []).some(hasAgentContent);
}

export interface SkillsChangeSummary {
  added: number;
  changed: number;
  removed: number;
  scripts: number;
}

/**
 * (C-1139) What changed in the skills of a pending version against the active one, over every app: skills added,
 * changed and removed, and the changed files that are scripts. `null` when no skill changed.
 */
export function skillsChangeSummary(plugin: PluginPackage): SkillsChangeSummary | null {
  const out: SkillsChangeSummary = { added: 0, changed: 0, removed: 0, scripts: 0 };
  for (const a of plugin.agent ?? []) {
    for (const s of a.changed?.skills ?? []) {
      out[s.change as "added" | "changed" | "removed"] += 1;
      const live = a.skills.find((k) => k.name === s.name);
      // A file that changed content and bit is listed twice (`changed` + `mode`): one script.
      const paths = new Set(s.files.filter((f) => f.change !== "removed").map((f) => f.path));
      for (const p of paths) if (live?.files.find((x) => x.path === p)?.executable) out.scripts += 1;
    }
  }
  return out.added + out.changed + out.removed === 0 ? null : out;
}

/** The one line above Approve: "Skills for agents changed: 1 added, 2 changed, 1 removed, 1 script", linking to the section. */
export function SkillsChangedLine({ plugin }: { plugin: PluginPackage }) {
  const s = skillsChangeSummary(plugin);
  if (!s) return null;
  const scripts = s.scripts > 0 ? `, ${s.scripts} ${s.scripts === 1 ? "script" : "scripts"}` : "";
  return (
    <p className="text-sm text-ink-2" data-testid="skills-changed-line">
      <a href={`#${AGENT_CONTENT_ID}`} className="text-link underline">
        Skills for agents changed
      </a>
      : {s.added} added, {s.changed} changed, {s.removed} removed{scripts}
    </p>
  );
}

function FileViewer({ pluginId, appId, file, label }: { pluginId: string; appId: string; file: AgentFileRef; label: string }) {
  const q = useAgentFile(pluginId, appId, file);
  if (q.isPending) return <Loading />;
  if (q.isError) return <ErrorState error={q.error} onRetry={() => q.refetch()} />;
  if (q.data.trim() === "") return <p className="text-muted">This file is empty.</p>;
  return <TextViewer label={`File ${label}`} text={q.data} />;
}

function FileButton({ id, selected, onSelect }: { id: string; selected: boolean; onSelect: () => void }) {
  return (
    <button
      type="button"
      aria-pressed={selected}
      className="min-h-11 break-all rounded-control text-left font-mono text-xs font-medium text-link underline sm:min-h-8"
      onClick={onSelect}
    >
      {id}
    </button>
  );
}

const H5 = "text-xs font-semibold uppercase tracking-label text-muted";

function AppAgent({ plugin, agent }: { plugin: PluginPackage; agent: PluginAgent }) {
  // One selected file per app; its text is shown under the skill (or the operations file) it belongs to.
  const [sel, setSel] = useState<{ id: string; ref: AgentFileRef } | null>(null);
  const name = plugin.apps.find((a) => a.app_id === agent.app_id)?.name ?? agent.app_id;
  const changed = agent.changed ?? null;
  const ops = agent.operations ?? null;
  const skillChange = new Map((changed?.skills ?? []).map((s) => [s.name, s]));
  const removedSkills = (changed?.skills ?? []).filter((s) => s.change === "removed");
  const viewer = (id: string) =>
    sel?.id === id ? <FileViewer key={id} pluginId={plugin.id} appId={agent.app_id} file={sel.ref} label={id} /> : null;

  return (
    <div className="min-w-0 space-y-3" data-testid={`agent-content-${agent.app_id}`}>
      <h4 className="font-medium text-ink">
        {name} <span className="font-mono text-xs font-normal text-muted">{agent.app_id}</span>
      </h4>
      {!hasAgentContent(agent) ? (
        <p className="text-muted">This app gives agents nothing but its description.</p>
      ) : (
        <>
          <p>
            <span className={H5}>Summary</span> {changed?.summary ? <Badge>Changed</Badge> : null}
            <br />
            {agent.summary ? (
              <span className="break-words text-ink">{agent.summary}</span>
            ) : (
              <span className="text-muted">No summary: the app description is used.</span>
            )}
          </p>
          {agent.instructions.trim() !== "" ? (
            <div>
              <h5 className={H5}>Essentials {changed?.instructions ? <Badge>Changed</Badge> : null}</h5>
              <TextViewer label={`Essentials of ${name}`} text={agent.instructions} className="mt-1 max-h-48" />
            </div>
          ) : null}
          {ops ? (
            <div>
              <h5 className={H5}>Operations {changed?.operations ? <Badge>Changed</Badge> : null}</h5>
              <div className="mt-1 flex flex-wrap items-center gap-x-3 gap-y-1">
                <FileButton id={ops.file} selected={sel?.id === ops.file} onSelect={() => setSel({ id: ops.file, ref: { kind: "operations" } })} />
                <span className="text-xs text-muted">
                  {ops.count} {ops.count === 1 ? "operation" : "operations"}, {formatBytes(ops.size)}
                </span>
              </div>
              <div className="mt-2">{viewer(ops.file)}</div>
            </div>
          ) : null}
          {agent.skills.length > 0 || removedSkills.length > 0 ? (
            <div className="space-y-2">
              <h5 className={H5}>Skills</h5>
              {agent.skills.map((s) => {
                const sc = skillChange.get(s.name);
                const fileChanges = sc?.change === "changed" ? sc.files : [];
                const removedFiles = fileChanges.filter((f) => f.change === "removed");
                const scripts = s.files.filter((f) => f.executable).length;
                return (
                  <Fold
                    key={s.name}
                    defaultOpen={sc != null}
                    data-testid={`agent-skill-${s.name}`}
                    summary={
                      <>
                        <span className="font-mono">{s.name}</span>{" "}
                        <span className="text-xs font-normal text-muted">{formatBytes(s.size)}</span>{" "}
                        {sc ? changeBadge(sc.change as Change, false) : null}
                        {scripts > 0 ? <Badge>{scripts === 1 ? "Script" : `${scripts} scripts`}</Badge> : null}
                        <span className="mt-0.5 block break-words text-xs font-normal text-ink-2">{s.description}</span>
                        {s.claimed_by ? (
                          <span className="mt-0.5 block text-xs font-normal text-muted" data-testid={`agent-skill-claimed-${s.name}`}>
                            Not given to agents: the name belongs to the {s.claimed_by.app_name} app
                          </span>
                        ) : null}
                      </>
                    }
                  >
                    <ul className="mt-2 divide-y divide-line rounded-control border border-line">
                      {s.files.map((f) => {
                        const id = `${s.name}/${f.path}`;
                        const marks = fileChanges.filter((c) => c.path === f.path);
                        return (
                          <li key={id}>
                            <div className="flex flex-wrap items-center gap-x-3 gap-y-1 px-3 py-2">
                              <FileButton
                                id={id}
                                selected={sel?.id === id}
                                onSelect={() => setSel({ id, ref: { kind: "skill", name: s.name, path: f.path === "SKILL.md" ? null : f.path } })}
                              />
                              <span className="text-xs text-muted">{formatBytes(f.size)}</span>
                              {f.executable ? <Badge>Script</Badge> : null}
                              {marks.map((m) => changeBadge(m.change as Change, f.executable))}
                            </div>
                            {sel?.id === id ? <div className="px-3 pb-3">{viewer(id)}</div> : null}
                          </li>
                        );
                      })}
                      {removedFiles.map((f) => (
                        <li key={`removed:${f.path}`} className="flex flex-wrap items-center gap-x-3 gap-y-1 px-3 py-2">
                          <span className="break-all font-mono text-xs text-ink-2">{`${s.name}/${f.path}`}</span>
                          {changeBadge("removed", false)}
                        </li>
                      ))}
                    </ul>
                  </Fold>
                );
              })}
              {removedSkills.map((s) => (
                <p key={s.name} className="flex flex-wrap items-center gap-2 px-1 text-sm">
                  <span className="break-all font-mono text-xs text-ink-2">{s.name}</span> {changeBadge("removed", false)}
                  <span className="text-xs text-muted">Not in this version.</span>
                </p>
              ))}
            </div>
          ) : null}
        </>
      )}
    </div>
  );
}

/**
 * (C-1139, `24 §9`) "For agents": what agents that use each app of the package get, as plain text only: the summary, the
 * essentials, the operations file and the skills with their files (scripts marked, changes marked against the active
 * version). Third-party text, never rendered as Markdown or HTML (SEC-150); a file is fetched when it is selected.
 */
export function AgentContentSection({ plugin }: { plugin: PluginPackage }) {
  const agents = plugin.agent ?? [];
  if (agents.length === 0) return <p className="text-muted">This package gives agents nothing but its description.</p>;
  return (
    <div className="space-y-5">
      <p className="text-ink">
        Agents that use this app get these skills. They are written by the publisher; scripts run in the agent&apos;s sandbox with the agent&apos;s
        access.
      </p>
      {agents.map((g) => (
        <AppAgent key={g.app_id} plugin={plugin} agent={g} />
      ))}
    </div>
  );
}
