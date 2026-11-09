"use client";

import { useState, type ChangeEvent, type FormEvent } from "react";

import { SizeMeter } from "@/components/memory/size-meter";
import { EmptyState, ErrorState, Loading } from "@/components/states";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Field, inputClass } from "@/components/ui/field";
import { Notice } from "@/components/ui/notice";
import { errorMessage } from "@/lib/api/errors";
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
  validateSkillName,
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
export function SkillsEditor({ scope, readOnly = false }: { scope: SkillScope; readOnly?: boolean }) {
  const agentList = useAgentSkills(scope.kind === "agent" ? scope.id : "");
  const wsList = useWorkspaceSkills(scope.kind === "workspace" ? scope.id : undefined);
  const orgList = useOrgSkills(scope.kind === "organization" ? scope.id : undefined);
  const list = scope.kind === "agent" ? agentList : scope.kind === "workspace" ? wsList : orgList;
  const [selected, setSelected] = useState<string | null>(null);
  const [creating, setCreating] = useState(false);
  const items = list.data?.items ?? [];
  const current = items.find((s) => s.id === selected);

  return (
    <div className="space-y-4">
      {readOnly ? (
        <Notice role="status">You can read these skills. Only workspace owners can change them.</Notice>
      ) : (
        <div className="flex justify-end">
          <Button type="button" onClick={() => { setCreating(true); setSelected(null); }}>New skill</Button>
        </div>
      )}
      {list.isPending ? <Loading /> : null}
      {list.isError ? <ErrorState error={list.error} onRetry={() => list.refetch()} /> : null}
      {list.data && items.length === 0 && !creating ? <EmptyState title="No skills yet" /> : null}
      <div className="grid gap-6 md:grid-cols-[16rem_1fr]">
        <ul aria-label="Skills" className="space-y-1">
          {items.map((s) => (
            <li key={s.id}>
              <button
                type="button"
                aria-current={selected === s.id}
                className="w-full rounded-control px-3 py-2 text-left text-sm text-ink hover:bg-subtle aria-[current=true]:bg-subtle aria-[current=true]:font-semibold"
                onClick={() => { setSelected(s.id); setCreating(false); }}
              >
                <span className="font-mono">{s.name}</span>
                {!s.enabled ? <span className="ml-1 text-xs text-muted">(disabled)</span> : null}
                <span className="block truncate text-xs font-normal text-ink-2">{s.description}</span>
              </button>
            </li>
          ))}
        </ul>
        <div>
          {creating ? <CreateSkill scope={scope} onDone={(id) => { setCreating(false); setSelected(id); }} /> : null}
          {!creating && current ? (
            <SkillEditor key={`${current.id}:${current.updated_at}`} scope={scope} skill={current} readOnly={readOnly} onDeleted={() => setSelected(null)} />
          ) : null}
          {!creating && !current ? <p className="text-sm text-muted">Select a skill to edit it.</p> : null}
        </div>
      </div>
    </div>
  );
}

function CreateSkill({ scope, onDone }: { scope: SkillScope; onDone: (id: string) => void }) {
  const create = useCreateSkill(scope);
  const [name, setName] = useState("");
  const [description, setDescription] = useState("");
  const [body, setBody] = useState("");
  const [err, setErr] = useState("");

  async function submit(e: FormEvent) {
    e.preventDefault();
    const issue = validateSkillName(name);
    if (issue) return setErr(issue);
    if (!description.trim() || description.length > SKILL_DESCRIPTION_MAX) return setErr(`Description is required, at most ${SKILL_DESCRIPTION_MAX} characters.`);
    if (body.length > SKILL_BODY_MAX) return setErr("The body is over the limit.");
    setErr("");
    try {
      const s = await create.mutateAsync({ name, description, body });
      onDone(s.id);
    } catch {
      // shown below
    }
  }
  return (
    <form onSubmit={(e) => void submit(e)} noValidate className="space-y-4 rounded-card border border-line bg-surface p-4 shadow-card sm:p-5">
      <Field id="sk-name" label="Name" hint="Immutable once created." error={err.includes("lowercase") || err.includes("reserved") ? err : undefined}>
        <input id="sk-name" className={inputClass} value={name} maxLength={SKILL_NAME_MAX} onChange={(e) => setName(e.target.value)} />
      </Field>
      <Field id="sk-desc" label="Description">
        <input id="sk-desc" className={inputClass} value={description} onChange={(e) => setDescription(e.target.value)} />
      </Field>
      <Field id="sk-body" label="SKILL.md">
        <textarea id="sk-body" className={`${inputClass} font-mono`} rows={10} value={body} onChange={(e) => setBody(e.target.value)} />
      </Field>
      <SizeMeter label="Body" used={body.length} max={SKILL_BODY_MAX} />
      {err && !err.includes("lowercase") && !err.includes("reserved") ? <Notice tone="danger" role="alert">{err}</Notice> : null}
      {create.error ? <Notice tone="danger" role="alert">{errorMessage(create.error)}</Notice> : null}
      <Button type="submit" size="lg" disabled={create.isPending}>Create skill</Button>
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

function SkillEditor({ scope, skill, readOnly = false, onDeleted }: { scope: SkillScope; skill: Skill; readOnly?: boolean; onDeleted: () => void }) {
  const update = useUpdateSkill(scope, skill.id);
  const del = useDeleteSkill(scope);
  const [description, setDescription] = useState(skill.description);
  const [body, setBody] = useState(skill.body);
  const [enabled, setEnabled] = useState(skill.enabled);
  const [confirmDel, setConfirmDel] = useState(false);
  const bodyOver = body.length > SKILL_BODY_MAX;
  const failed = update.error ?? del.error;

  function submit(e: FormEvent) {
    e.preventDefault();
    if (bodyOver || !description.trim() || description.length > SKILL_DESCRIPTION_MAX) return;
    update.mutate({ description, body, enabled });
  }

  return (
    <div className="space-y-6">
      <OverriddenIn skill={skill} />
      <form onSubmit={submit} noValidate className="space-y-4 rounded-card border border-line bg-surface p-4 shadow-card sm:p-5">
        <Field id="sk-name" label="Name" hint="The name cannot be changed.">
          <input id="sk-name" className={inputClass} value={skill.name} readOnly />
        </Field>
        <Field id="sk-desc" label="Description" error={description.length > SKILL_DESCRIPTION_MAX ? `At most ${SKILL_DESCRIPTION_MAX} characters.` : undefined}>
          <input id="sk-desc" className={inputClass} value={description} readOnly={readOnly} onChange={(e) => setDescription(e.target.value)} />
        </Field>
        <Field id="sk-body" label="SKILL.md">
          <textarea id="sk-body" className={`${inputClass} font-mono`} rows={10} value={body} readOnly={readOnly} onChange={(e) => setBody(e.target.value)} />
        </Field>
        <SizeMeter label="Body" used={body.length} max={SKILL_BODY_MAX} />
        <label className="flex items-center gap-2 text-sm font-medium text-ink">
          <input type="checkbox" className="size-4 accent-accent" checked={enabled} disabled={readOnly} onChange={(e) => setEnabled(e.target.checked)} /> Enabled
        </label>
        {failed ? <Notice tone="danger" role="alert">{errorMessage(failed)}</Notice> : null}
        {readOnly ? null : (
          <div className="flex flex-wrap gap-2">
            <Button type="submit" size="lg" disabled={bodyOver || update.isPending}>Save</Button>
            {confirmDel ? (
              <>
                <Button type="button" variant="danger" onClick={() => void del.mutateAsync(skill.id).then(onDeleted)}>Confirm delete</Button>
                <Button type="button" variant="ghost" onClick={() => setConfirmDel(false)}>Cancel</Button>
              </>
            ) : (
              <Button type="button" variant="danger" onClick={() => setConfirmDel(true)}>Delete</Button>
            )}
          </div>
        )}
      </form>
      <SkillFiles scope={scope} skill={skill} readOnly={readOnly} />
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
      <h3 className="font-display text-base font-semibold text-ink">Files</h3>
      <SizeMeter label="Total size" used={skill.total_size} max={SKILL_TOTAL_MAX} />
      {skill.files.length === 0 ? <p className="text-sm text-muted">No extra files.</p> : <FileTree nodes={buildTree(skill.files)} readOnly={readOnly} onDelete={(p) => del.mutate(p)} onReplace={replace} />}
      {del.error || put.error ? <Notice tone="danger" role="alert">{errorMessage(del.error ?? put.error)}</Notice> : null}
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
        <label className="flex items-center gap-2 text-sm text-ink">
          <input type="checkbox" className="size-4 accent-accent" checked={exec} onChange={(e) => setExec(e.target.checked)} /> Executable
        </label>
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
              <span className="text-xs text-muted">{n.file.size.toLocaleString("en-US")} chars{n.file.executable ? ", executable" : ""}</span>
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
  return (
    <section aria-label="Effective skills" className="space-y-3">
      <h3 className="font-display text-base font-semibold text-ink">Skills this agent receives</h3>
      <p className="text-xs text-muted">
        One skill per name. The agent&apos;s own skill takes precedence over a workspace skill, which takes precedence over an organization skill. A
        disabled skill never hides a lower one. Workspace skills are managed by workspace owners and organization skills by organization owners and admins.
      </p>
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
