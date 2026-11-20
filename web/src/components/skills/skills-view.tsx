"use client";

import { useState, type ChangeEvent, type FormEvent } from "react";

import { formatCount } from "@/lib/format-date";

import { SizeMeter } from "@/components/memory/size-meter";
import { EmptyState, ErrorState, Loading } from "@/components/states";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Altitude } from "@/components/ui/altitude";
import { ConfirmAction } from "@/components/ui/confirm-dialog";
import { Field, inputClass } from "@/components/ui/field";
import { CheckboxField } from "@/components/ui/form";
import { ErrorNotice, Notice } from "@/components/ui/notice";
import { SwitchField } from "@/components/ui/switch";
import { ApiError } from "@/lib/api/client";
import { newSkillName, skillDisplayName } from "@/lib/skill-name";
import type { BudgetTier } from "@/lib/api/skill-libraries";
import { AddFromLibrary, LibraryBadge, LibraryManagedNotice, LibrarySkillActions, PendingUpdatesNotice } from "./library";
import {
  SKILL_BODY_MAX,
  SKILL_DESCRIPTION_MAX,
  SKILL_NAME_MAX,
  SKILL_TOTAL_MAX,
  buildTree,
  useAgentSkills,
  useCreateSkill,
  useDeleteSkill,
  useDeleteSkillFile,
  useEffectiveSkills,
  useOrgSkills,
  usePutSkillFile,
  useUpdateSkill,
  useWorkspaceSkills,
  validateSkillPath,
  TIER_LABEL,
  type FileTreeNode,
  type Skill,
  type SkillScope,
  type SkillTier,
} from "@/lib/api/skills";

const TIER_TONE: Record<SkillTier, "accent" | "neutral"> = { agent: "accent", workspace: "neutral", org: "neutral" };

/** A pill naming the tier a skill belongs to; the text carries the meaning. */
export function TierBadge({ tier }: { tier: SkillTier }) {
  return <Badge tone={TIER_TONE[tier]}>{TIER_LABEL[tier]}</Badge>;
}

/**
 * Editable list + editor for one tier (agent, workspace or organization). `readOnly` is the member view of a
 * workspace tier: the list and every skill open, nothing can be changed (writes are `can_own`, the API enforces it).
 */
export function SkillsEditor({ scope, readOnly = false, orgId, agentName }: { scope: SkillScope; readOnly?: boolean; orgId?: string; agentName?: string }) {
  const agentList = useAgentSkills(scope.kind === "agent" ? scope.id : "");
  const wsList = useWorkspaceSkills(scope.kind === "workspace" ? scope.id : undefined);
  const orgList = useOrgSkills(scope.kind === "organization" ? scope.id : undefined);
  const list = scope.kind === "agent" ? agentList : scope.kind === "workspace" ? wsList : orgList;
  const [selected, setSelected] = useState<string | null>(null);
  const [creating, setCreating] = useState(false);
  const items = list.data?.items ?? [];
  const current = items.find((s) => s.id === selected);
  const tier: BudgetTier = scope.kind === "organization" ? "org" : scope.kind;
  // Library-sourced content of this tier counts toward its budget (docs/10 §Git skill library).
  const libraryItems = items.filter((s) => s.source && !s.source.detached);
  const usage = { bytes: libraryItems.reduce((n, s) => n + s.total_size, 0), count: libraryItems.length };

  return (
    <div className="space-y-4">
      {readOnly ? (
        <Notice role="status">You can read these skills. Only workspace owners can change them.</Notice>
      ) : (
        <div className="flex flex-wrap justify-end gap-2">
          <AddFromLibrary scope={scope} orgId={orgId} tier={tier} usage={usage} onAdded={(id) => { setCreating(false); setSelected(id); }} />
          {/* The empty state carries its own New skill (one action). */}
          {items.length > 0 || creating ? <Button type="button" onClick={() => { setCreating(true); setSelected(null); }}>New skill</Button> : null}
        </div>
      )}
      <PendingUpdatesNotice scope={scope} enabled={!readOnly && libraryItems.length > 0} />
      {list.isPending ? <Loading /> : null}
      {list.isError ? <ErrorState error={list.error} onRetry={() => list.refetch()} /> : null}
      {list.data && items.length === 0 && !creating ? (
        <EmptyState
          title="No skills yet"
          when="Skills are instructions an agent can load when a task needs them."
          action={
            readOnly ? undefined : (
              <Button type="button" onClick={() => { setCreating(true); setSelected(null); }}>
                New skill
              </Button>
            )
          }
        />
      ) : null}
      {/* (P5-POLISH-6, C6) While the list is empty there is no detail pane and no "Select a skill" text. */}
      <div className={items.length === 0 && !creating ? "hidden" : "grid gap-6 md:layout-aside-md"}>
        <ul aria-label="Skills" className="min-w-0 space-y-1">
          {items.map((s) => (
            <li key={s.id} className="min-w-0">
              <button
                type="button"
                aria-current={selected === s.id}
                className="block w-full min-w-0 overflow-hidden rounded-control px-3 py-2 text-left text-sm text-ink hover:bg-subtle aria-[current=true]:bg-subtle aria-[current=true]:font-semibold"
                onClick={() => { setSelected(s.id); setCreating(false); }}
              >
                <span className="font-medium">{skillDisplayName(s.name)}</span>
                {!s.enabled ? <span className="ml-1 text-xs text-muted">(off)</span> : null}
                <span className="block truncate text-xs font-normal text-ink-2">{s.description}</span>
                {s.source ? (
                  <span className="mt-1 flex flex-wrap gap-1">
                    <LibraryBadge source={s.source} />
                  </span>
                ) : null}
              </button>
            </li>
          ))}
        </ul>
        <div className="min-w-0">
          {creating ? <CreateSkill scope={scope} taken={items.map((s) => s.name)} onDone={(id) => { setCreating(false); setSelected(id); }} onCancel={() => setCreating(false)} /> : null}
          {!creating && current ? (
            <SkillEditor key={`${current.id}:${current.updated_at}`} scope={scope} skill={current} readOnly={readOnly} agentName={agentName} onDeleted={() => setSelected(null)} />
          ) : null}
          {!creating && !current && items.length > 0 ? <p className="text-sm text-muted">Select a skill to edit it.</p> : null}
        </div>
      </div>
    </div>
  );
}

/** Who a skill is "on for" in words, by tier. */
function onFor(scope: SkillScope, agentName?: string): string {
  if (scope.kind === "agent") return agentName ? `On for ${agentName}` : "On for this agent";
  return scope.kind === "workspace" ? "On for this workspace's agents" : "On for the organization's agents";
}

/**
 * (P5-REFINE-12, behaviour-2) A new skill: a name in words, "Use it when…" and the instructions. The folder name is
 * Aurora's: made from the Name, unique, never `aurora-…`; a 409 retries once with the next suffix.
 */
function CreateSkill({ scope, taken, onDone, onCancel }: { scope: SkillScope; taken: string[]; onDone: (id: string) => void; onCancel: () => void }) {
  const create = useCreateSkill(scope);
  const [name, setName] = useState("");
  const [description, setDescription] = useState("");
  const [body, setBody] = useState("");
  const [err, setErr] = useState<{ name?: string; description?: string; body?: string }>({});

  async function submit(e: FormEvent) {
    e.preventDefault();
    const next: typeof err = {};
    if (!name.trim() || name.length > SKILL_NAME_MAX) next.name = "Give the skill a name, up to 64 characters.";
    if (!description.trim()) next.description = "Say when the agent should use it.";
    else if (description.length > SKILL_DESCRIPTION_MAX) next.description = `Keep it to ${SKILL_DESCRIPTION_MAX} characters.`;
    if (body.length > SKILL_BODY_MAX) next.body = `That is too long: instructions hold up to ${formatCount(SKILL_BODY_MAX)} characters.`;
    setErr(next);
    if (Object.keys(next).length) return;
    for (let attempt = 0; ; attempt++) {
      try {
        const s = await create.mutateAsync({ name: newSkillName(name, taken, attempt), description: description.trim(), body });
        onDone(s.id);
        return;
      } catch (error) {
        if (attempt >= 1 || !(error instanceof ApiError && error.status === 409)) return;
      }
    }
  }
  return (
    <form onSubmit={(e) => void submit(e)} noValidate className="space-y-4 rounded-card border border-line bg-surface p-4 shadow-card sm:p-5">
      <Field id="sk-name" label="Name" error={err.name}>
        <input id="sk-name" className={inputClass} value={name} maxLength={SKILL_NAME_MAX} placeholder="Weekly report" onChange={(e) => setName(e.target.value)} />
      </Field>
      <Field id="sk-desc" label="Use it when…" error={err.description}>
        <input id="sk-desc" className={inputClass} value={description} placeholder="someone asks for a weekly status report" onChange={(e) => setDescription(e.target.value)} />
      </Field>
      <Field id="sk-body" label="Instructions" error={err.body}>
        <textarea id="sk-body" className={inputClass} rows={8} value={body} onChange={(e) => setBody(e.target.value)} />
      </Field>
      <SizeMeter label="Size" used={body.length} max={SKILL_BODY_MAX} />
      <ErrorNotice error={create.error} />
      <div className="flex flex-wrap gap-2">
        <Button type="submit" disabled={create.isPending}>Save</Button>
        <Button type="button" variant="ghost" onClick={onCancel}>Cancel</Button>
      </div>
    </form>
  );
}

/** Agents whose own enabled skill of the same name overrides a workspace skill (`14`). */
function OverriddenIn({ skill }: { skill: Skill }) {
  const list = skill.overridden_in ?? [];
  if (skill.tier !== "workspace" || list.length === 0) return null;
  return (
    <Notice role="status">
      Overridden by an agent skill of the same name in:{" "}
      {list.map((o, i) => (
        <span key={o.skill_id}>
          {i > 0 ? ", " : ""}
          <span className="font-mono">{o.agent.handle}</span>
        </span>
      ))}
      . Those agents do not receive this skill.
    </Notice>
  );
}

/** A skill that reaches an agent from another tier, opened read-only (agent tab, `14`). */
export function SkillReadOnly({ skill }: { skill: Skill }) {
  return (
    <div className="space-y-4">
      <div className="flex items-center gap-2">
        <span className="font-mono text-sm font-semibold text-ink">{skill.name}</span>
        <TierBadge tier={skill.tier} />
        {!skill.enabled ? <Badge>Disabled</Badge> : null}
      </div>
      <SkillEditor key={`${skill.id}:${skill.updated_at}`} scope={{ kind: "organization", id: skill.org_id }} skill={skill} readOnly onDeleted={() => undefined} />
    </div>
  );
}

function SkillEditor({ scope, skill, readOnly = false, agentName, onDeleted }: { scope: SkillScope; skill: Skill; readOnly?: boolean; agentName?: string; onDeleted: () => void }) {
  const update = useUpdateSkill(scope, skill.id);
  const del = useDeleteSkill(scope);
  const [description, setDescription] = useState(skill.description);
  const [body, setBody] = useState(skill.body);
  const [enabled, setEnabled] = useState(skill.enabled);
  const bodyOver = body.length > SKILL_BODY_MAX;
  const failed = update.error ?? del.error;
  // (P4-GSL) A library skill is pinned and read-only: only enable/disable, detach and remove are offered.
  const managed = !!skill.source && !skill.source.detached;
  const contentReadOnly = readOnly || managed;
  const tier = skill.tier as BudgetTier;

  function submit(e: FormEvent) {
    e.preventDefault();
    if (managed) {
      update.mutate({ enabled });
      return;
    }
    if (bodyOver || !description.trim() || description.length > SKILL_DESCRIPTION_MAX) return;
    update.mutate({ description, body, enabled });
  }

  return (
    <div className="space-y-6">
      <OverriddenIn skill={skill} />
      {managed ? <LibraryManagedNotice /> : null}
      {skill.source?.detached ? (
        <p className="flex flex-wrap items-center gap-2">
          <LibraryBadge source={skill.source} />
        </p>
      ) : null}
      {managed ? <LibrarySkillActions skill={skill} tier={tier} canDecide={!readOnly} /> : null}
      <form onSubmit={submit} noValidate className="space-y-4 rounded-card border border-line bg-surface p-4 shadow-card sm:p-5">
        <Field id="sk-name" label="Name" hint="The name cannot be changed.">
          <input id="sk-name" className={inputClass} value={skillDisplayName(skill.name)} readOnly />
        </Field>
        <Field id="sk-desc" label="Use it when…" error={description.length > SKILL_DESCRIPTION_MAX ? `At most ${SKILL_DESCRIPTION_MAX} characters.` : undefined}>
          <input id="sk-desc" className={inputClass} value={description} readOnly={contentReadOnly} onChange={(e) => setDescription(e.target.value)} />
        </Field>
        <Field id="sk-body" label="Instructions">
          <textarea id="sk-body" className={inputClass} rows={8} value={body} readOnly={contentReadOnly} onChange={(e) => setBody(e.target.value)} />
        </Field>
        <SizeMeter label="Size" used={body.length} max={SKILL_BODY_MAX} />
        <SwitchField id="skill-enabled" label={onFor(scope, agentName)} checked={enabled} disabled={readOnly} onChange={setEnabled} />
        <ErrorNotice error={failed} />
        {readOnly ? null : (
          <div className="flex flex-wrap gap-2">
            <Button type="submit" disabled={bodyOver || update.isPending}>Save</Button>
            <ConfirmAction
              label={managed ? "Remove" : "Delete"}
              title={`${managed ? "Remove" : "Delete"} the skill “${skillDisplayName(skill.name)}”?`}
              description={
                managed
                  ? "Agents stop using it. It can be installed again from the library."
                  : "Agents stop using it and its files are deleted. This can't be undone."
              }
              confirmLabel={managed ? "Remove skill" : "Delete skill"}
              onConfirm={() => void del.mutateAsync(skill.id).then(onDeleted)}
            />
          </div>
        )}
      </form>
      {/* (P5-REFINE-12) Files, scripts and the folder name are Expert: a collapsed line stands in below it. */}
      <Altitude id="skill.files" level="expert" label="Advanced: files and scripts">
        <div className="space-y-3">
          <h3 className="font-display text-base font-semibold text-ink">Advanced: files and scripts</h3>
          <p className="text-xs text-muted">
            Folder name: <code className="font-mono">{skill.name}</code>. The instructions above are its <code className="font-mono">SKILL.md</code>.
          </p>
          <SkillFiles scope={scope} skill={skill} readOnly={contentReadOnly} />
        </div>
      </Altitude>
    </div>
  );
}

function SkillFiles({ scope, skill, readOnly = false }: { scope: SkillScope; skill: Skill; readOnly?: boolean }) {
  const put = usePutSkillFile(scope, skill.id);
  const del = useDeleteSkillFile(scope, skill.id);
  const [path, setPath] = useState("");
  const [exec, setExec] = useState(false);
  const [file, setFile] = useState<File | null>(null);
  const [text, setText] = useState("");
  const [err, setErr] = useState("");

  async function add(e: FormEvent) {
    e.preventDefault();
    const p = path.trim();
    const issue = validateSkillPath(p);
    if (issue) return setErr(issue);
    setErr("");
    const content = file ? new Uint8Array(await file.arrayBuffer()) : new TextEncoder().encode(text);
    try {
      await put.mutateAsync({ path: p, content, executable: exec });
      setPath("");
      setText("");
      setFile(null);
    } catch {
      // shown below
    }
  }
  const replace = (p: string, executable: boolean) => (e: ChangeEvent<HTMLInputElement>) => {
    const f = e.target.files?.[0];
    if (!f) return;
    void f.arrayBuffer().then((b) => put.mutate({ path: p, content: new Uint8Array(b), executable }));
  };

  return (
    <section aria-label="Files" className="space-y-3">
      <h4 className="text-sm font-semibold text-ink">Files</h4>
      <SizeMeter label="Total size" used={skill.total_size} max={SKILL_TOTAL_MAX} />
      {skill.files.length === 0 ? <p className="text-sm text-muted">No extra files.</p> : <FileTree nodes={buildTree(skill.files)} readOnly={readOnly} onDelete={(p) => del.mutate(p)} onReplace={replace} />}
      {del.error || put.error ? <ErrorNotice error={del.error ?? put.error} /> : null}
      {readOnly ? null : <form onSubmit={(e) => void add(e)} noValidate className="space-y-3 rounded-card border border-line bg-surface p-4 shadow-card">
        <Field id="sf-path" label="File path" hint="Relative, for example scripts/run.sh" error={err || undefined}>
          <input id="sf-path" className={inputClass} value={path} onChange={(e) => setPath(e.target.value)} />
        </Field>
        <Field id="sf-upload" label="Upload a file (optional)">
          <input id="sf-upload" type="file" onChange={(e) => setFile(e.target.files?.[0] ?? null)} />
        </Field>
        {!file ? (
          <Field id="sf-text" label="Or type text content">
            <textarea id="sf-text" className={`${inputClass} font-mono`} rows={4} value={text} onChange={(e) => setText(e.target.value)} />
          </Field>
        ) : null}
        <CheckboxField id="skill-file-executable" label="Executable" checked={exec} onChange={setExec} />
        <Button type="submit" size="sm" disabled={put.isPending}>Save file</Button>
      </form>}
    </section>
  );
}

function FileTree({ nodes, readOnly = false, onDelete, onReplace }: { nodes: FileTreeNode[]; readOnly?: boolean; onDelete: (path: string) => void; onReplace: (path: string, exec: boolean) => (e: ChangeEvent<HTMLInputElement>) => void }) {
  return (
    <ul className="space-y-1 pl-3 text-sm text-ink">
      {nodes.map((n) => (
        <li key={n.path}>
          {n.file ? (
            <div className="flex flex-wrap items-center gap-2">
              <span className="font-mono">{n.name}</span>
              <span className="text-xs text-muted">{formatCount(n.file.size)} chars{n.file.executable ? ", executable" : ""}</span>
              {readOnly ? null : (
                <>
                  <label className="text-xs text-link underline">
                    {n.file.executable ? "Replace (keep executable)" : "Replace"}
                    <input type="file" className="sr-only" aria-label={`Replace ${n.path}`} onChange={onReplace(n.path, n.file.executable)} />
                  </label>
                  <label className="text-xs text-link underline">
                    {n.file.executable ? "Replace as not executable" : "Replace as executable"}
                    <input type="file" className="sr-only" aria-label={`Replace ${n.path} as ${n.file.executable ? "not executable" : "executable"}`} onChange={onReplace(n.path, !n.file.executable)} />
                  </label>
                  <Button type="button" size="sm" variant="ghost" aria-label={`Delete ${n.path}`} onClick={() => onDelete(n.path)}>Delete</Button>
                </>
              )}
            </div>
          ) : (
            <>
              <span className="font-mono text-ink-2">{n.name}/</span>
              <FileTree nodes={n.children} readOnly={readOnly} onDelete={onDelete} onReplace={onReplace} />
            </>
          )}
        </li>
      ))}
    </ul>
  );
}

/**
 * What the agent actually receives (agent tab): one skill per name with its source tier and what it shadows,
 * then the skills that do not reach it (shadowed or disabled). Workspace and organization skills open read-only.
 */
export function EffectiveSkillsView({ agentId }: { agentId: string }) {
  const eff = useEffectiveSkills(agentId);
  const [open, setOpen] = useState<Skill | null>(null);
  const items = eff.data?.items ?? [];
  const shadowed = eff.data?.shadowed ?? [];
  const row = (s: Skill, extra: React.ReactNode) => (
    <li key={s.id} className="flex flex-wrap items-center gap-x-2 gap-y-1 py-2 text-sm text-ink">
      <button type="button" className="font-mono text-link underline" aria-label={`Open ${s.name} (${TIER_LABEL[s.tier]})`} onClick={() => setOpen(s)}>
        {s.name}
      </button>
      <TierBadge tier={s.tier} />
      {extra}
      <span className="basis-full truncate text-xs text-ink-2">{s.description}</span>
    </li>
  );
  // (P5-POLISH-6, C6) "Skills this agent receives" shows only when there are some.
  if (eff.data && items.length === 0 && shadowed.length === 0) return null;
  return (
    <section aria-label="Effective skills" className="space-y-3">
      <h3 className="font-display text-base font-semibold text-ink">Skills this agent receives</h3>
      <Altitude id="skill.files" level="expert">
        <p className="text-xs text-muted">
          One skill per name. The agent&apos;s own skill takes precedence over a workspace skill, which takes precedence over an organization skill. A
          disabled skill never hides a lower one. Workspace skills are managed by workspace owners and organization skills by organization owners and admins.
        </p>
      </Altitude>
      {eff.isPending ? <Loading /> : null}
      {eff.isError ? <ErrorState error={eff.error} onRetry={() => eff.refetch()} /> : null}
      {eff.data && items.length === 0 ? <p className="text-sm text-muted">None.</p> : null}
      <ul aria-label="Received skills" className="divide-y divide-line">
        {items.map((i) =>
          row(
            i.skill,
            i.shadows.length > 0 ? (
              <span className="text-xs text-muted">Overrides {i.shadows.map((s) => TIER_LABEL[s.tier].toLowerCase()).join(" and ")} skill</span>
            ) : null,
          ),
        )}
      </ul>
      {shadowed.length > 0 ? (
        <>
          <h4 className="text-sm font-semibold text-ink">Not applied</h4>
          <ul aria-label="Skills not applied" className="divide-y divide-line">
            {shadowed.map((i) =>
              row(
                i.skill,
                <span className="text-xs text-muted">
                  {!i.enabled ? "Disabled" : i.shadowed_by ? `Shadowed by the ${TIER_LABEL[i.shadowed_by.tier].toLowerCase()} skill` : ""}
                </span>,
              ),
            )}
          </ul>
        </>
      ) : null}
      {open ? (
        <div className="space-y-3 rounded-card border border-line bg-surface p-4 shadow-card">
          <SkillReadOnly skill={open} />
          <Button type="button" variant="ghost" onClick={() => setOpen(null)}>Close</Button>
        </div>
      ) : null}
    </section>
  );
}
