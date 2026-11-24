import { exportFiles, slug, type AgentExport } from "./agent-export";
import { parseYaml, stringifyYaml, type YamlValue } from "./yaml";
import { writeZip, type ZipEntry } from "./zip";

/**
 * (P6-TPL-5, C-1118) **Save as template**: the agent export with `template.yaml` added, rooted at
 * `<id>/` so it can be unzipped into a template repository as it is (C-1103) and pushed. Built on
 * `exportFiles` — one zip and YAML writer. The agent's library-sourced skills become `skills:`
 * references (C-1142), never copies; its helper files are kept byte for byte. Never in the file:
 * accounts, connection ids, secrets, webhook addresses, the AI key and library tokens.
 */

export interface TemplateMeta {
  /** The C-1141 rule: `[a-z0-9]+(-[a-z0-9]+)*`, at most 64; also the zip's root directory. */
  id: string;
  /** Semver without build metadata, like `1.0.0`. */
  version: string;
  /** 1..80; the suggested agent name. */
  name: string;
  /** 1..280. */
  summary: string;
  /** The dialog's "What it's for" text; becomes `README.md`. */
  readme: string;
}

/** One `apps:` entry, written to `template.yaml`. */
export interface TemplateAppEntry {
  app: string;
  required: boolean;
  /** Read, ask or act — or `custom`, which must carry `tools` (the level is then not written). */
  level: "read" | "ask" | "act" | "custom";
  /** A custom choice: `tools` replaces `level` (C-1141). */
  tools?: { allow: string[]; ask: string[] };
  /** 1..200. */
  why: string;
}

/** One `skills:` reference to a library skill (C-1142): the library's address, not its content. */
export interface TemplateSkillRef {
  library: string;
  skill: string;
  ref?: string;
  path?: string;
  required: boolean;
  /** 1..200; the skill's description, cut to 200. */
  why: string;
}

/** One of the agent's helper files (C-1143), kept byte for byte under `helpers/`. */
export interface TemplateHelper {
  path: string;
  content: Uint8Array;
  executable: boolean;
}

/** An error whose message is one sentence for the person. */
export class TemplateError extends Error {}

/** The C-1141 rules, as one sentence per problem. */
export const TEMPLATE_ID_RE = /^[a-z0-9]+(-[a-z0-9]+)*$/;
export const TEMPLATE_VERSION_RE = /^\d+\.\d+\.\d+$/;

/** The limits of C-1141 (see `crates/aurora-core/src/limits.rs`), for the dialog's checks. */
export const TEMPLATE_LIMITS = {
  memories: 20,
  skills: 25,
  skillRefs: 20,
  helpers: 50,
  helperFile: 256 * 1024,
  helpersTotal: 2 * 1024 * 1024,
  automations: 10,
  apps: 10,
  files: 200,
  unpacked: 8 * 1024 * 1024,
  archive: 5 * 1024 * 1024,
  readme: 20_000,
  summary: 280,
  why: 200,
  name: 80,
  id: 64,
} as const;

/** A text the placeholders would be read in ({input.x}, {agent.name}, {person.name}), with its file. */
export function placeholderProblem(text: string): string | null {
  const m = /\{(?:input\.[a-zA-Z0-9_]+|agent\.name|person\.name)\}/.exec(text);
  return m ? `it contains the placeholder “${m[0]}”, which a template fills in` : null;
}

// ---- template.yaml -------------------------------------------------------------------------------------------------

/** The `template.yaml` document (C-1141: strict — unknown keys are refused, so only these are written). */
export function templateYaml(meta: TemplateMeta, apps: TemplateAppEntry[], skillRefs: TemplateSkillRef[]): YamlValue {
  return {
    format: 1,
    id: meta.id,
    version: meta.version,
    name: meta.name,
    summary: meta.summary,
    ...(apps.length ? { apps: apps.map(appYaml) } : {}),
    ...(skillRefs.length ? { skills: skillRefs.map(skillRefYaml) } : {}),
  };
}

function appYaml(a: TemplateAppEntry): YamlValue {
  return {
    app: a.app,
    required: a.required,
    // `tools` replaces `level` by a custom choice; `level` must be read, ask or act.
    ...(a.tools ? { tools: { allow: [...a.tools.allow], ask: [...a.tools.ask] } } : a.level === "custom" ? {} : { level: a.level }),
    why: a.why,
  };
}

function skillRefYaml(r: TemplateSkillRef): YamlValue {
  return {
    library: r.library,
    ...(r.ref ? { ref: r.ref } : {}),
    ...(r.path ? { path: r.path } : {}),
    skill: r.skill,
    required: r.required,
    why: r.why,
  };
}

// ---- the files -----------------------------------------------------------------------------------------------------

const enc = new TextEncoder();
const dec = new TextDecoder("utf-8", { fatal: true });

const CLOSING = (day: string) => `Made with Save as template on ${day}. Accounts, secrets and conversations are not included.\n`;

function isRecurring(a: AgentExport["automations"][number]): boolean {
  return a.kind === "webhook" || "cron" in a.cadence;
}

/**
 * The files of a saved template, under `<id>/`: the export's files of the ticked groups (the caller
 * filters the export), `template.yaml` first, the README from **What it's for**, schedules without
 * `time_zone`, pending one-time schedules already left out, and the helpers with their executable
 * bit. A date is the day it was made.
 */
export function templateFiles(e: AgentExport, meta: TemplateMeta, apps: TemplateAppEntry[], skillRefs: TemplateSkillRef[], helpers: TemplateHelper[], now: Date = new Date()): ZipEntry[] {
  const files = exportFiles({ ...e, model: "default", automations: e.automations.filter(isRecurring) }, meta.id, now);
  // `exportFiles` slugs the handle; an id may be a character longer, so the root is renamed exactly.
  const root = `${slug(meta.id)}/`;
  const out: ZipEntry[] = [{ name: `${meta.id}/template.yaml`, data: enc.encode(stringifyYaml(templateYaml(meta, apps, skillRefs))) }];
  for (const f of files) {
    const rel = f.name.startsWith(root) ? f.name.slice(root.length) : f.name;
    const entry: ZipEntry = { name: `${meta.id}/${rel}`, data: f.data };
    if (rel === "README.md") {
      entry.data = enc.encode(`${meta.readme.trimEnd()}\n\n${CLOSING(now.toISOString().slice(0, 10))}`);
    } else if (/^automations\/[^/]+\.yaml$/.test(rel)) {
      // A schedule runs in the workspace's time zone (C-1141): `time_zone` is not carried.
      const text = new TextDecoder().decode(f.data);
      const v = parseYaml(text);
      if (v !== null && typeof v === "object" && !Array.isArray(v)) {
        delete v.time_zone;
        const filled = typeof v.prompt === "string" ? v.prompt : typeof v.instructions === "string" ? v.instructions : null;
        if (filled !== null) checkPlaceholder(filled, rel);
        entry.data = enc.encode(stringifyYaml(v));
      }
    } else if (rel === "instructions.md") {
      checkPlaceholder(dec.decode(f.data), rel);
    } else if (/^memory\/[^/]+\.md$/.test(rel)) {
      checkPlaceholder(dec.decode(f.data), rel);
    } else if (/^skills\/[^/]+\/SKILL\.md$/.test(rel)) {
      checkPlaceholder(dec.decode(f.data), rel);
    }
    out.push(entry);
  }
  for (const h of helpers) {
    if (h.executable && !isUtf8(h.content)) {
      throw new TemplateError(`Refused: the helper “${h.path}” is executable, so it must be text.`);
    }
    out.push({ name: `${meta.id}/helpers/${h.path}`, data: h.content, executable: h.executable });
  }
  return out;
}

function checkPlaceholder(text: string, file: string): void {
  const problem = placeholderProblem(text);
  if (problem) throw new TemplateError(`Refused: “${file}” — ${problem}.`);
}

function isUtf8(bytes: Uint8Array): boolean {
  try {
    dec.decode(bytes);
    return !bytes.includes(0);
  } catch {
    return false;
  }
}

/** `<id>-<version>.zip` and its bytes. */
export function templateZip(e: AgentExport, meta: TemplateMeta, apps: TemplateAppEntry[], skillRefs: TemplateSkillRef[], helpers: TemplateHelper[], now: Date = new Date()): { fileName: string; bytes: Uint8Array } {
  return { fileName: `${meta.id}-${meta.version}.zip`, bytes: writeZip(templateFiles(e, meta, apps, skillRefs, helpers, now), now) };
}

// ---- apps from the agent's access ----------------------------------------------------------------------------------

/** One row of the dialog's app list: the level (or the custom tool lists) one app would get. */
export interface AppAccessRow {
  app: string;
  level: "read" | "ask" | "act" | "custom";
  tools?: { allow: string[]; ask: string[] };
}

/** What `appAccessRows` reads: the shapes of the accounts list (C-917) and the grants route (C-504). */
export interface TemplateAccountRow {
  kind: string;
  app: { app_id: string } | null;
  ref: { id: string; type: string };
  agents: { agent: { id: string }; level: "read" | "ask" | "act" | "custom" | "use" }[];
}

export interface TemplateGrantRow {
  agent_id: string;
  connection_id: string;
  mode: "full" | "custom";
  allow_tools: string[];
  ask_tools: string[];
}

const LEVEL_RANK = { read: 0, ask: 1, custom: 2, act: 3 } as const;

/**
 * One row per app the agent has access to, from the workspace accounts list read for this agent
 * (C-917: `agents[].level` is the server's read/ask/act classification). A `custom` (or `use`) row's
 * tool lists come from the agent's grant for that connection (C-504: `mode: full` is `act`,
 * `custom` carries `allow`/`ask`). Several accounts of one app merge into one row — an app id may
 * appear in at most one `apps:` entry (C-1141) — the strongest level wins.
 */
export function appAccessRows(accounts: TemplateAccountRow[], grants: TemplateGrantRow[], agentId: string): AppAccessRow[] {
  const byApp = new Map<string, AppAccessRow>();
  for (const a of accounts) {
    if (!a.app) continue; // an AI key or a website is never an app entry
    const mine = a.agents.find((x) => x.agent.id === agentId);
    if (!mine) continue;
    const app = a.app.app_id;
    let row: AppAccessRow | null = null;
    if (mine.level === "read" || mine.level === "ask" || mine.level === "act") {
      row = { app, level: mine.level };
    } else {
      const g = grants.find((x) => x.agent_id === agentId && x.connection_id === a.ref.id);
      if (g && g.mode === "full") row = { app, level: "act" };
      else if (g) row = { app, level: "custom", tools: { allow: [...g.allow_tools], ask: [...g.ask_tools] } };
    }
    if (!row) continue;
    const prev = byApp.get(app);
    if (!prev || LEVEL_RANK[row.level] > LEVEL_RANK[prev.level]) byApp.set(app, row);
  }
  return [...byApp.values()].sort((x, y) => x.app.localeCompare(y.app));
}
