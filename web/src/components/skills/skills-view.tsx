"use client";

import { useState, type ChangeEvent, type FormEvent } from "react";

import { SizeMeter } from "@/components/memory/size-meter";
import { EmptyState, ErrorState, Loading } from "@/components/states";
import { Button } from "@/components/ui/button";
import { Field, inputClass } from "@/components/ui/field";
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
  useOrgSkills,
  usePutSkillFile,
  useUpdateSkill,
  validateSkillName,
  validateSkillPath,
  type FileTreeNode,
  type Skill,
  type SkillScope,
} from "@/lib/api/skills";

/** Editable list + editor for one tier (agent or organization). */
export function SkillsEditor({ scope }: { scope: SkillScope }) {
  const agentList = useAgentSkills(scope.kind === "agent" ? scope.id : "");
  const orgList = useOrgSkills(scope.kind === "organization" ? scope.id : undefined);
  const list = scope.kind === "agent" ? agentList : orgList;
  const [selected, setSelected] = useState<string | null>(null);
  const [creating, setCreating] = useState(false);
  const items = list.data?.items ?? [];
  const current = items.find((s) => s.id === selected);

  return (
    <div className="space-y-4">
      <div className="flex justify-end">
        <Button type="button" onClick={() => { setCreating(true); setSelected(null); }}>New skill</Button>
      </div>
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
                className="w-full rounded px-2 py-1.5 text-left text-sm hover:bg-neutral-50 aria-[current=true]:bg-neutral-100"
                onClick={() => { setSelected(s.id); setCreating(false); }}
              >
                <span className="font-mono">{s.name}</span>
                {!s.enabled ? <span className="ml-1 text-xs text-neutral-500">(disabled)</span> : null}
                <span className="block truncate text-xs text-neutral-600">{s.description}</span>
              </button>
            </li>
          ))}
        </ul>
        <div>
          {creating ? <CreateSkill scope={scope} onDone={(id) => { setCreating(false); setSelected(id); }} /> : null}
          {!creating && current ? (
            <SkillEditor key={`${current.id}:${current.updated_at}`} scope={scope} skill={current} onDeleted={() => setSelected(null)} />
          ) : null}
          {!creating && !current ? <p className="text-sm text-neutral-500">Select a skill to edit it.</p> : null}
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
    <form onSubmit={(e) => void submit(e)} noValidate className="space-y-3">
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
      {err && !err.includes("lowercase") && !err.includes("reserved") ? <p role="alert" className="text-sm text-red-700">{err}</p> : null}
      {create.error ? <p role="alert" className="text-sm text-red-700">{errorMessage(create.error)}</p> : null}
      <Button type="submit" disabled={create.isPending}>Create skill</Button>
    </form>
  );
}

function SkillEditor({ scope, skill, onDeleted }: { scope: SkillScope; skill: Skill; onDeleted: () => void }) {
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
      <form onSubmit={submit} noValidate className="space-y-3">
        <Field id="sk-name" label="Name" hint="The name cannot be changed.">
          <input id="sk-name" className={inputClass} value={skill.name} readOnly />
        </Field>
        <Field id="sk-desc" label="Description" error={description.length > SKILL_DESCRIPTION_MAX ? `At most ${SKILL_DESCRIPTION_MAX} characters.` : undefined}>
          <input id="sk-desc" className={inputClass} value={description} onChange={(e) => setDescription(e.target.value)} />
        </Field>
        <Field id="sk-body" label="SKILL.md">
          <textarea id="sk-body" className={`${inputClass} font-mono`} rows={10} value={body} onChange={(e) => setBody(e.target.value)} />
        </Field>
        <SizeMeter label="Body" used={body.length} max={SKILL_BODY_MAX} />
        <label className="flex items-center gap-2 text-sm">
          <input type="checkbox" checked={enabled} onChange={(e) => setEnabled(e.target.checked)} /> Enabled
        </label>
        {failed ? <p role="alert" className="text-sm text-red-700">{errorMessage(failed)}</p> : null}
        <div className="flex gap-2">
          <Button type="submit" disabled={bodyOver || update.isPending}>Save</Button>
          {confirmDel ? (
            <>
              <Button type="button" variant="outline" onClick={() => void del.mutateAsync(skill.id).then(onDeleted)}>Confirm delete</Button>
              <Button type="button" variant="ghost" onClick={() => setConfirmDel(false)}>Cancel</Button>
            </>
          ) : (
            <Button type="button" variant="outline" onClick={() => setConfirmDel(true)}>Delete</Button>
          )}
        </div>
      </form>
      <SkillFiles scope={scope} skill={skill} />
    </div>
  );
}

function SkillFiles({ scope, skill }: { scope: SkillScope; skill: Skill }) {
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
      <h3 className="text-sm font-medium">Files</h3>
      <SizeMeter label="Total size" used={skill.total_size} max={SKILL_TOTAL_MAX} />
      {skill.files.length === 0 ? <p className="text-sm text-neutral-500">No extra files.</p> : <FileTree nodes={buildTree(skill.files)} onDelete={(p) => del.mutate(p)} onReplace={replace} />}
      {del.error || put.error ? <p role="alert" className="text-sm text-red-700">{errorMessage(del.error ?? put.error)}</p> : null}
      <form onSubmit={(e) => void add(e)} noValidate className="space-y-2 rounded-md border border-neutral-200 p-3">
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
        <label className="flex items-center gap-2 text-sm">
          <input type="checkbox" checked={exec} onChange={(e) => setExec(e.target.checked)} /> Executable
        </label>
        <Button type="submit" size="sm" disabled={put.isPending}>Save file</Button>
      </form>
    </section>
  );
}

function FileTree({ nodes, onDelete, onReplace }: { nodes: FileTreeNode[]; onDelete: (path: string) => void; onReplace: (path: string, exec: boolean) => (e: ChangeEvent<HTMLInputElement>) => void }) {
  return (
    <ul className="space-y-1 pl-3 text-sm">
      {nodes.map((n) => (
        <li key={n.path}>
          {n.file ? (
            <div className="flex flex-wrap items-center gap-2">
              <span className="font-mono">{n.name}</span>
              <span className="text-xs text-neutral-500">{n.file.size.toLocaleString("en-US")} chars{n.file.executable ? ", executable" : ""}</span>
              <label className="text-xs underline">
                {n.file.executable ? "Replace (keep executable)" : "Replace"}
                <input type="file" className="sr-only" aria-label={`Replace ${n.path}`} onChange={onReplace(n.path, n.file.executable)} />
              </label>
              <label className="text-xs underline">
                {n.file.executable ? "Replace as not executable" : "Replace as executable"}
                <input type="file" className="sr-only" aria-label={`Replace ${n.path} as ${n.file.executable ? "not executable" : "executable"}`} onChange={onReplace(n.path, !n.file.executable)} />
              </label>
              <Button type="button" size="sm" variant="ghost" aria-label={`Delete ${n.path}`} onClick={() => onDelete(n.path)}>Delete</Button>
            </div>
          ) : (
            <>
              <span className="font-mono text-neutral-600">{n.name}/</span>
              <FileTree nodes={n.children} onDelete={onDelete} onReplace={onReplace} />
            </>
          )}
        </li>
      ))}
    </ul>
  );
}

/** Read-only list of organization skills (shown on the agent tab). */
export function OrgSkillsReadOnly({ orgId }: { orgId: string }) {
  const list = useOrgSkills(orgId);
  return (
    <section aria-label="Organization skills" className="space-y-2">
      <h3 className="text-sm font-medium">Organization skills</h3>
      <p className="text-xs text-neutral-500">Managed by organization owners and admins. An agent skill with the same name takes precedence.</p>
      {list.isPending ? <Loading /> : null}
      {list.isError ? <ErrorState error={list.error} onRetry={() => list.refetch()} /> : null}
      {list.data && list.data.items.length === 0 ? <p className="text-sm text-neutral-500">None.</p> : null}
      <ul className="space-y-1">
        {list.data?.items.map((s) => (
          <li key={s.id} className="text-sm">
            <span className="font-mono">{s.name}</span> <span className="text-neutral-600">{s.description}</span>
          </li>
        ))}
      </ul>
    </section>
  );
}
