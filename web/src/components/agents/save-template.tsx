"use client";

import { useEffect, useState } from "react";

import { Button } from "@/components/ui/button";
import { Dialog, DialogClose, DialogContent } from "@/components/ui/dialog";
import { CheckboxField } from "@/components/ui/form";
import { Field, inputClass } from "@/components/ui/field";
import { Markdown } from "@/components/ui/markdown";
import { ErrorNotice, Notice } from "@/components/ui/notice";
import { api } from "@/lib/api/client";
import { unwrap } from "@/lib/api/unwrap";
import { appAccessRows, TEMPLATE_ID_RE, TEMPLATE_LIMITS, TEMPLATE_VERSION_RE, TemplateError, templateFiles, templateZip, type AppAccessRow, type TemplateAppEntry, type TemplateHelper, type TemplateMeta, type TemplateSkillRef } from "@/lib/export/agent-template";
import { slug, type AgentExport } from "@/lib/export/agent-export";
import { loadAgentExport } from "@/lib/export/agent-import";
import type { Agent } from "@/lib/api/queries";

/**
 * (P6-TPL-5, C-1118) **Save as template…** beside Export on the Manage page. Reads the agent through
 * the existing routes and builds the template zip in the browser (`lib/export/agent-template.ts`):
 * the export with `template.yaml`, the "What it's for" README, library skills as references and the
 * helper files. **Save** downloads `<id>-<version>.zip`; it publishes nothing.
 */

type SaveAgent = Pick<Agent, "id" | "name" | "handle" | "brief" | "model_id" | "proactive_messaging" | "org_id" | "workspace_id" | "avatar_url">;

interface Loaded {
  e: AgentExport;
  omitted: string[];
  rows: AppAccessRow[];
  helpers: TemplateHelper[];
  refs: TemplateSkillRef[];
  /** Library skills the caller could not read: left out, with a note. */
  unreadable: string[];
  /** Organization and workspace skills: never written. */
  orgSkills: number;
  /** Pending one-time schedules: never written (C-1141 refuses them). */
  oneTime: number;
}

async function readHelper(agentId: string, path: string): Promise<Uint8Array> {
  const { data, error, response } = await api.GET("/v1/agents/{agent}/helpers/content", {
    params: { path: { agent: agentId }, query: { path } },
    parseAs: "arrayBuffer",
  });
  if (error !== undefined || !response.ok) throw new Error(`Could not read the helper “${path}”.`);
  return new Uint8Array(data as ArrayBuffer);
}

/** The agent as a template's inputs: rows from the accounts list (C-917) and grants (C-504), library skill references. */
async function loadSaveTemplate(agent: SaveAgent): Promise<Loaded> {
  const path = { params: { path: { agent: agent.id } } };
  const [exported, grants, accounts, helpers, skills, effective] = await Promise.all([
    loadAgentExport(agent),
    unwrap(api.GET("/v1/agents/{agent}/grants", path)),
    unwrap(api.GET("/v1/workspaces/{ws}/accounts", { params: { path: { ws: agent.workspace_id }, query: { agent_id: agent.id } } })),
    unwrap(api.GET("/v1/agents/{agent}/helpers", path)),
    unwrap(api.GET("/v1/agents/{agent}/skills", path)),
    unwrap(api.GET("/v1/agents/{agent}/skills/effective", path)),
  ]);
  const rows = appAccessRows(accounts.items, grants.connections, agent.id);
  const helperFiles: TemplateHelper[] = await Promise.all(helpers.items.map(async (h) => ({ path: h.path, executable: h.executable, content: await readHelper(agent.id, h.path) })));
  // Library-sourced skills become references (C-1142); their content is never written.
  const library = skills.items.filter((s) => s.source?.kind === "library" && !s.source.detached && s.source.library_id);
  const refs: TemplateSkillRef[] = [];
  const unreadable: string[] = [];
  await Promise.all(
    library.map(async (s) => {
      try {
        const lib = await unwrap(api.GET("/v1/skill-libraries/{id}", { params: { path: { id: s.source!.library_id! } }, query: { org: agent.org_id } }));
        if (!lib.url) throw new Error("no url");
        refs.push({
          library: lib.url,
          ...(lib.ref ? { ref: lib.ref } : {}),
          ...(lib.path ? { path: lib.path } : {}),
          skill: s.name,
          required: true,
          why: s.description.slice(0, TEMPLATE_LIMITS.why),
        });
      } catch {
        unreadable.push(s.name);
      }
    }),
  );
  const agentTierNames = new Set(skills.items.map((s) => s.name));
  return {
    e: { ...exported.data, skills: exported.data.skills.filter((s) => !library.some((l) => l.name === s.name)) },
    omitted: exported.omitted,
    rows,
    helpers: helperFiles,
    refs,
    unreadable,
    orgSkills: effective.items.filter((x) => (x.tier === "workspace" || x.tier === "org") && !x.app && !agentTierNames.has(x.skill.name)).length,
    oneTime: exported.data.automations.filter((a) => a.kind === "schedule" && !("cron" in a.cadence)).length,
  };
}

/** The outline of "What it's for" (D-458, C-1118): the placeholder line must be replaced before Save. */
export function readmeOutline(name: string): string {
  return `## What ${name} does\n\n${outlinePlaceholder(name)}\n\n## Who it's for\n\n## What it needs\n\n## What it does on its own\n\n## Adapting it\n`;
}

export const outlinePlaceholder = (name: string) => `Say what ${name} is for, so people can decide whether to use it.`;

const LEVEL_LABEL = { read: "read", ask: "ask", act: "act" } as const;
const levelLabel = (row: AppAccessRow) =>
  row.tools
    ? `custom (${row.tools.allow.length} allowed, ${row.tools.ask.length} on approval)`
    : row.level === "custom"
      ? "custom"
      : LEVEL_LABEL[row.level];

interface AppDraft {
  required: boolean;
  why: string;
}

interface Choices {
  instructions: boolean;
  memories: boolean;
  skills: boolean;
  helpers: boolean;
  schedules: boolean;
  webhooks: boolean;
}

/** The first client-side refusal (C-1118), or null. */
function saveProblem(loaded: Loaded, choices: Choices, drafts: AppDraft[], meta: TemplateMeta): string | null {
  if (meta.name.length < 1 || meta.name.length > TEMPLATE_LIMITS.name) return "The name is 1 to 80 characters.";
  if (!TEMPLATE_ID_RE.test(meta.id) || meta.id.length > TEMPLATE_LIMITS.id) return "The id is lowercase words joined by dashes, at most 64 characters.";
  if (!TEMPLATE_VERSION_RE.test(meta.version)) return "The version is a semver without build metadata, like 1.0.0.";
  if (meta.readme.trim().length < 1) return "What it's for is empty: say what the agent is designed for.";
  if (meta.readme.length > TEMPLATE_LIMITS.readme) return `What it's for is longer than ${TEMPLATE_LIMITS.readme} characters.`;
  if (meta.readme.includes(outlinePlaceholder(meta.name))) return `Replace the placeholder lines: ${outlinePlaceholder(meta.name)}`;
  if (meta.summary.length < 1 || meta.summary.length > TEMPLATE_LIMITS.summary) return "The summary is 1 to 280 characters.";
  if (loaded.rows.length > TEMPLATE_LIMITS.apps) return `The template has more than ${TEMPLATE_LIMITS.apps} apps.`;
  for (const [i, row] of loaded.rows.entries()) {
    if (drafts[i]!.why.length < 1 || drafts[i]!.why.length > TEMPLATE_LIMITS.why) return `Say why ${row.app} is needed (1 to ${TEMPLATE_LIMITS.why} characters).`;
  }
  if (choices.memories && loaded.e.memories.length > TEMPLATE_LIMITS.memories) return `The template has more than ${TEMPLATE_LIMITS.memories} memories.`;
  if (choices.skills && loaded.e.skills.length > TEMPLATE_LIMITS.skills) return `The template has more than ${TEMPLATE_LIMITS.skills} skills.`;
  if (loaded.refs.length > TEMPLATE_LIMITS.skillRefs) return `The template has more than ${TEMPLATE_LIMITS.skillRefs} library skill references.`;
  if (loaded.helpers.length > TEMPLATE_LIMITS.helpers) return `The agent has more than ${TEMPLATE_LIMITS.helpers} helper files.`;
  if (loaded.helpers.some((h) => h.content.length > TEMPLATE_LIMITS.helperFile)) return "A helper file is larger than 256 KiB.";
  if (loaded.helpers.reduce((n, h) => n + h.content.length, 0) > TEMPLATE_LIMITS.helpersTotal) return "The helper files together are larger than 2 MB.";
  if (loaded.e.automations.length > TEMPLATE_LIMITS.automations) return `The template has more than ${TEMPLATE_LIMITS.automations} automations.`;
  return null;
}

/** Builds the zip's inputs from the dialog state; throws `TemplateError` for a placeholder or a binary helper. */
function buildInputs(loaded: Loaded, choices: Choices, drafts: AppDraft[]) {
  const e: AgentExport = {
    ...loaded.e,
    instructions: choices.instructions ? loaded.e.instructions : "",
    memories: choices.memories ? loaded.e.memories : [],
    skills: choices.skills ? loaded.e.skills : [],
    automations: loaded.e.automations.filter((a) => (a.kind === "schedule" ? choices.schedules : choices.webhooks)),
  };
  const apps: TemplateAppEntry[] = loaded.rows.map((row, i) => ({
    app: row.app,
    required: drafts[i]!.required,
    level: row.level,
    ...(row.tools ? { tools: row.tools } : {}),
    why: drafts[i]!.why,
  }));
  const refs = choices.skills ? loaded.refs : [];
  const helpers = choices.helpers ? loaded.helpers : [];
  return { e, apps, refs, helpers };
}

export function SaveTemplateDialog({ agent, onClose }: { agent: SaveAgent; onClose: () => void }) {
  const [loaded, setLoaded] = useState<Loaded | null>(null);
  const [error, setError] = useState<unknown>(null);
  const [name, setName] = useState(agent.name);
  const [idEdited, setIdEdited] = useState(false);
  const [idValue, setIdValue] = useState(slug(agent.name));
  const id = idEdited ? idValue : slug(name);
  const [version, setVersion] = useState("1.0.0");
  const [summary, setSummary] = useState("");
  const [readme, setReadme] = useState(readmeOutline(agent.name));
  const [preview, setPreview] = useState(false);
  const [choices, setChoices] = useState<Choices>({ instructions: true, memories: false, skills: true, helpers: true, schedules: true, webhooks: true });
  const [drafts, setDrafts] = useState<AppDraft[]>([]);
  const [problem, setProblem] = useState<string | null>(null);

  useEffect(() => {
    let live = true;
    loadSaveTemplate(agent).then(
      (r) => {
        if (!live) return;
        setLoaded(r);
        setChoices((c) => ({ ...c, instructions: r.e.instructions.trim() !== "", helpers: r.helpers.length > 0, skills: r.e.skills.length > 0 || r.refs.length > 0, schedules: r.e.automations.some((a) => a.kind === "schedule"), webhooks: r.e.automations.some((a) => a.kind === "webhook") }));
        setDrafts(r.rows.map(() => ({ required: true, why: "" })));
      },
      (e: unknown) => live && setError(e),
    );
    return () => {
      live = false;
    };
  }, [agent]);

  function save() {
    if (!loaded) return;
    setProblem(null);
    const meta: TemplateMeta = { id, version, name, summary, readme };
    const p = saveProblem(loaded, choices, drafts, meta);
    if (p) return setProblem(p);
    try {
      const { e, apps, refs, helpers } = buildInputs(loaded, choices, drafts);
      const files = templateFiles(e, meta, apps, refs, helpers);
      if (files.length > TEMPLATE_LIMITS.files) return setProblem(`The template has more than ${TEMPLATE_LIMITS.files} files.`);
      const unpacked = files.reduce((n, f) => n + f.data.length, 0);
      if (unpacked > TEMPLATE_LIMITS.unpacked) return setProblem("The template is larger than 8 MB once unpacked.");
      const { fileName, bytes } = templateZip(e, meta, apps, refs, helpers);
      if (bytes.length > TEMPLATE_LIMITS.archive) return setProblem("The zip is larger than 5 MB.");
      const url = URL.createObjectURL(new Blob([bytes as BlobPart], { type: "application/zip" }));
      const a = document.createElement("a");
      a.href = url;
      a.download = fileName;
      document.body.appendChild(a);
      a.click();
      a.remove();
      setTimeout(() => URL.revokeObjectURL(url), 1000);
      onClose();
    } catch (e) {
      setProblem(e instanceof TemplateError || e instanceof Error ? e.message : "The template could not be built.");
    }
  }

  const leftOut: string[] = loaded ? [
    ...(loaded.orgSkills > 0 ? [`${loaded.orgSkills} organization or workspace ${loaded.orgSkills === 1 ? "skill is" : "skills are"} not included.`] : []),
    ...(loaded.oneTime > 0 ? [`The ${loaded.oneTime === 1 ? "one-time schedule is" : `${loaded.oneTime} one-time schedules are`} not included: a template carries recurring schedules only.`] : []),
    ...loaded.unreadable.map((n) => `The library skill “${n}” could not be read, so it is left out.`),
    ...loaded.omitted,
  ] : [];
  const ticking = loaded
    ? [
        `Instructions${choices.instructions ? "" : " (left out)"}`,
        `${loaded.e.memories.length} ${loaded.e.memories.length === 1 ? "memory" : "memories"}`,
        `${loaded.e.skills.length} own ${loaded.e.skills.length === 1 ? "skill" : "skills"}`,
        `${loaded.refs.length} library ${loaded.refs.length === 1 ? "skill" : "skills"} (references)`,
        `${loaded.helpers.length} helper ${loaded.helpers.length === 1 ? "file" : "files"}`,
        pluralCount(loaded.e.automations.filter((a) => a.kind === "schedule").length, "schedule"),
        pluralCount(loaded.e.automations.filter((a) => a.kind === "webhook").length, "webhook"),
      ].filter((s) => !s.startsWith("0 "))
    : [];

  return (
    <Dialog open onOpenChange={(o) => !o && onClose()}>
      <DialogContent
        title={`Save ${agent.name} as a template`}
        description="A zip rooted at the template's id, to unzip into a template repository and push. Accounts, connection ids, secrets and conversations are not included."
        size="lg"
        footer={
          <div className="flex flex-wrap items-center justify-end gap-2">
            <DialogClose asChild>
              <Button variant="outline">Cancel</Button>
            </DialogClose>
            <Button disabled={!loaded} onClick={save}>
              Save
            </Button>
          </div>
        }
      >
        <div className="space-y-4 text-sm text-ink-2">
          {error ? <ErrorNotice error={error} /> : null}
          {!loaded && !error ? <p role="status">Reading {agent.name}…</p> : null}
          {loaded ? (
            <>
              <div className="grid gap-3 sm:grid-cols-2">
                <Field id="tpl-name" label="Template name">
                  <input id="tpl-name" className={inputClass} value={name} onChange={(e) => setName(e.target.value)} />
                </Field>
                <Field id="tpl-id" label="Id" hint="Lowercase words joined by dashes, at most 64 characters.">
                  <input id="tpl-id" className={inputClass} value={id} onChange={(e) => { setIdEdited(true); setIdValue(e.target.value); }} />
                </Field>
                <Field id="tpl-version" label="Version">
                  <input id="tpl-version" className={inputClass} value={version} onChange={(e) => setVersion(e.target.value)} />
                </Field>
                <Field id="tpl-summary" label="Summary" hint="One line for the gallery.">
                  <input id="tpl-summary" className={inputClass} value={summary} onChange={(e) => setSummary(e.target.value)} />
                </Field>
              </div>
              <div className="space-y-1.5">
                <div className="flex items-center justify-between">
                  <label htmlFor="tpl-readme" className="text-sm font-medium text-ink">
                    What it&apos;s for
                  </label>
                  <Button type="button" variant="ghost" size="sm" onClick={() => setPreview(!preview)}>
                    {preview ? "Edit" : "Preview"}
                  </Button>
                </div>
                {preview ? (
                  <div className="rounded-control border border-line-strong p-3" data-testid="readme-preview">
                    <Markdown source={readme} />
                  </div>
                ) : (
                  <textarea id="tpl-readme" className={`${inputClass} min-h-48`} rows={8} value={readme} onChange={(e) => setReadme(e.target.value)} />
                )}
                <p className="text-xs text-muted">Shown to the people deciding whether to deploy it. Markdown.</p>
              </div>
              <fieldset className="space-y-2" data-testid="template-groups">
                <legend className="text-sm font-medium text-ink">What the file carries</legend>
                <CheckboxField id="tpl-instructions" label="Instructions" checked={choices.instructions} onChange={(v) => setChoices({ ...choices, instructions: v })} />
                <CheckboxField
                  id="tpl-memories"
                  label={`${loaded.e.memories.length} ${loaded.e.memories.length === 1 ? "memory" : "memories"}`}
                  hint={`Memories can hold what ${agent.name} learned about people. Check them before you share this file.`}
                  checked={choices.memories}
                  onChange={(v) => setChoices({ ...choices, memories: v })}
                />
                <CheckboxField id="tpl-skills" label={`${pluralSkills(loaded)}${choices.skills && loaded.refs.length > 0 ? " · a library skill is a reference to its library" : ""}`} checked={choices.skills} onChange={(v) => setChoices({ ...choices, skills: v })} />
                {loaded.helpers.length > 0 ? <CheckboxField id="tpl-helpers" label={`${loaded.helpers.length} helper ${loaded.helpers.length === 1 ? "file" : "files"}`} checked={choices.helpers} onChange={(v) => setChoices({ ...choices, helpers: v })} /> : null}
                {loaded.e.automations.some((a) => a.kind === "schedule") ? <CheckboxField id="tpl-schedules" label="Schedules" checked={choices.schedules} onChange={(v) => setChoices({ ...choices, schedules: v })} /> : null}
                {loaded.e.automations.some((a) => a.kind === "webhook") ? <CheckboxField id="tpl-webhooks" label="Webhooks" hint="Created paused, with a new secret." checked={choices.webhooks} onChange={(v) => setChoices({ ...choices, webhooks: v })} /> : null}
                <p className="text-xs text-muted" data-testid="template-summary-line">
                  {ticking.join(" · ")}
                </p>
              </fieldset>
              {loaded.rows.length > 0 ? (
                <fieldset className="space-y-2" data-testid="template-apps">
                  <legend className="text-sm font-medium text-ink">Apps {agent.name} uses</legend>
                  {loaded.rows.map((row, i) => (
                    <div key={row.app} className="space-y-1.5 rounded-card border border-line-strong p-3">
                      <div className="flex flex-wrap items-center justify-between gap-2">
                        <p className="font-medium text-ink">
                          {row.app} <span className="text-muted">· {levelLabel(row)}</span>
                        </p>
                        <label className="flex items-center gap-2 text-sm">
                          <input type="checkbox" checked={drafts[i]!.required} onChange={(e) => setDrafts(drafts.map((d, j) => (j === i ? { ...d, required: e.target.checked } : d)))} />
                          Required
                        </label>
                      </div>
                      <Field id={`tpl-why-${row.app}`} label="Why it's needed">
                        <input id={`tpl-why-${row.app}`} className={inputClass} value={drafts[i]!.why} onChange={(e) => setDrafts(drafts.map((d, j) => (j === i ? { ...d, why: e.target.value } : d)))} />
                      </Field>
                    </div>
                  ))}
                  <p className="text-xs text-muted">An app entry is a suggestion: the person creating from the template grants the access (nothing is carried in the file).</p>
                </fieldset>
              ) : null}
              {leftOut.map((line) => (
                <Notice key={line} tone="neutral" role="status">
                  {line}
                </Notice>
              ))}
              <p className="text-xs text-muted">The file has no inputs; add them by hand (see the publisher guide). Publishing: put this folder in a template repository and push.</p>
            </>
          ) : null}
          {problem ? (
            <Notice tone="danger" role="alert" data-testid="save-problem">
              {problem}
            </Notice>
          ) : null}
        </div>
      </DialogContent>
    </Dialog>
  );
}

function pluralSkills(loaded: Loaded): string {
  const own = loaded.e.skills.length;
  const refs = loaded.refs.length;
  if (own === 0 && refs === 0) return "Skills";
  if (refs === 0) return `${own} ${own === 1 ? "skill" : "skills"}`;
  if (own === 0) return `${refs} library ${refs === 1 ? "skill" : "skills"}`;
  return `${own} ${own === 1 ? "skill" : "skills"} · ${refs} library ${refs === 1 ? "skill" : "skills"}`;
}

const pluralCount = (n: number, one: string) => `${n} ${n === 1 ? one : `${one}s`}`;
