import type { AgentTemplate } from "@/lib/onboarding/templates";

import { parseYaml, stringifyYaml, YamlError, type YamlValue } from "./yaml";
import { nameProblem, ZipError, writeZip, type ZipEntry } from "./zip";

/**
 * (P5-REFINE-19, D-413) What leaves Aurora when someone exports an agent, and what an import reads. A purpose template
 * (`lib/onboarding/templates.ts`) **is** an export with `purpose` set (`templateAsExport`): one data shape.
 *
 * ```
 * <handle>/
 *   agent.yaml            format, name, purpose?, model, may_message_first
 *   instructions.md       the instructions text
 *   avatar.png|jpg|webp   the agent's avatar, when it has one (at most one, 512 KiB)
 *   memory/<key>.md       front matter `key`, `updated`, `locked?` (true only when locked); then the text
 *   skills/<name>/SKILL.md  front matter `name`, `use-it-when`; then the body (+ the skill's other files)
 *   automations/<name>.yaml  a schedule or a webhook, without address or secret
 *   README.md             where it came from
 * ```
 *
 * Never in an export: ids, secrets, webhook addresses, accounts, channels, conversations, the AI key.
 */

export const EXPORT_FORMAT = 1;

export interface ExportMemory {
  key: string;
  updated: string;
  content: string;
  /** (P6-MEMLOCK, C-1150) Written only when true, so existing exports are unchanged. */
  locked?: boolean;
}

export interface ExportSkillFile {
  path: string;
  content: Uint8Array;
}

export interface ExportSkill {
  name: string;
  /** (C-1173) The display name, written as `title` only when the skill has one. */
  title?: string;
  useItWhen: string;
  body: string;
  /** The skill's files other than SKILL.md, as stored (read with `GET /v1/skills/{id}/files/{path}`, C-1099). */
  files: ExportSkillFile[];
}

/** A recurring schedule's cron, or a pending one-time schedule's local `run_at` (`YYYY-MM-DDTHH:MM[:SS]`, read in `time_zone`). */
export type ExportCadence = { cron: string } | { preset: "once"; run_at: string };

/** The local date-time the API takes for a one-time schedule (C-1092). */
export const RUN_AT_RE = /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}(:\d{2})?$/;

export type ExportAutomation =
  | { kind: "schedule"; name: string; prompt: string; cadence: ExportCadence; time_zone: string; enabled: boolean }
  // `instructions` is what the agent does when the webhook is called (not a secret); the address and the secret are never carried.
  | { kind: "webhook"; name: string; instructions: string; verification: "signed" | "password" | "none"; enabled: boolean };

/** (C-41, C-1141) The avatar's file type and bytes. The API stores a PNG, so an export from an agent is `png`; a file may carry any of the three. */
export type AvatarExt = "png" | "jpg" | "webp";
export interface ExportAvatar {
  ext: AvatarExt;
  bytes: Uint8Array;
}

/** Mirrors `AVATAR_MAX` (512 KiB): the server stays authoritative. */
export const EXPORT_AVATAR_MAX = 512 * 1024;
const AVATAR_FILE_RE = /^avatar\.(png|jpg|webp)$/;

export interface AgentExport {
  format: number;
  name: string;
  purpose?: string;
  /** `default` or the model's name. */
  model: string;
  may_message_first: boolean;
  instructions: string;
  memories: ExportMemory[];
  skills: ExportSkill[];
  automations: ExportAutomation[];
  /** Written as `avatar.<ext>` only when the agent has one. */
  avatar?: ExportAvatar;
}

/** The `agent.yaml` document of an export. */
export function agentYaml(e: AgentExport): YamlValue {
  return {
    format: e.format,
    name: e.name,
    ...(e.purpose ? { purpose: e.purpose } : {}),
    model: e.model,
    may_message_first: e.may_message_first,
  };
}

/** A name safe for a file: lowercase words joined by `-`; `item` when nothing is left. */
export function slug(text: string): string {
  const s = text
    .normalize("NFKD")
    .replace(/[̀-ͯ]/g, "")
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "")
    .slice(0, 60);
  return s || "item";
}

function automationYaml(a: ExportAutomation): YamlValue {
  return a.kind === "schedule"
    ? { kind: "schedule", name: a.name, prompt: a.prompt, cadence: "cron" in a.cadence ? { cron: a.cadence.cron } : { preset: "once", run_at: a.cadence.run_at }, time_zone: a.time_zone, enabled: a.enabled }
    : { kind: "webhook", name: a.name, instructions: a.instructions, verification: a.verification, enabled: a.enabled };
}

const enc = new TextEncoder();

function frontMatter(fields: Record<string, string | boolean>, text: string): string {
  return `---\n${stringifyYaml(fields)}---\n${text}`;
}

/** The files of an export, under `<handle>/`. A date is the day it was made. */
export function exportFiles(e: AgentExport, handle: string, now: Date = new Date()): ZipEntry[] {
  const root = slug(handle);
  const files: ZipEntry[] = [];
  const add = (path: string, text: string | Uint8Array) => files.push({ name: `${root}/${path}`, data: typeof text === "string" ? enc.encode(text) : text });
  add("agent.yaml", stringifyYaml(agentYaml(e)));
  add("instructions.md", e.instructions);
  if (e.avatar) add(`avatar.${e.avatar.ext}`, e.avatar.bytes);
  // (P6-MEMLOCK, C-1150) `locked: true` only on a locked memory: no line for an unlocked one.
  for (const m of e.memories) add(`memory/${m.key}.md`, frontMatter({ key: m.key, updated: m.updated, ...(m.locked ? { locked: true } : {}) }, m.content));
  for (const s of e.skills) {
    add(`skills/${s.name}/SKILL.md`, frontMatter({ name: s.name, ...(s.title ? { title: s.title } : {}), "use-it-when": s.useItWhen }, s.body));
    for (const f of s.files) add(`skills/${s.name}/${f.path}`, f.content);
  }
  const used = new Set<string>();
  for (const a of e.automations) {
    let file = slug(a.name);
    for (let i = 2; used.has(file); i++) file = `${slug(a.name)}-${i}`;
    used.add(file);
    add(`automations/${file}.yaml`, stringifyYaml(automationYaml(a)));
  }
  const day = now.toISOString().slice(0, 10);
  add("README.md", `Exported from Aurora on ${day}. Secrets, accounts and conversations are not included.\n`);
  return files;
}

/** `<handle>-<date>.zip` and its bytes. */
export function exportZip(e: AgentExport, handle: string, now: Date = new Date()): { fileName: string; bytes: Uint8Array } {
  return { fileName: `${slug(handle)}-${now.toISOString().slice(0, 10)}.zip`, bytes: writeZip(exportFiles(e, handle, now), now) };
}

/** A purpose template as an export: the same shape, `purpose` set, nothing else carried. */
export function templateAsExport(t: AgentTemplate): AgentExport {
  return {
    format: EXPORT_FORMAT,
    name: t.nameSuggestion,
    purpose: t.id,
    model: "default",
    may_message_first: false,
    instructions: t.instructions,
    memories: [],
    skills: [],
    automations: t.schedule ? [{ kind: "schedule", name: t.label, prompt: t.schedule.prompt, cadence: { cron: t.schedule.cron }, time_zone: "UTC", enabled: false }] : [],
  };
}

/** The template an export would make (what `templateAsExport` reads back), for the equality test and for templates from files. */
export function exportAsTemplate(e: AgentExport, label: string): Pick<AgentTemplate, "id" | "label" | "nameSuggestion" | "instructions" | "schedule"> {
  const schedule = e.automations.find((a) => a.kind === "schedule" && "cron" in a.cadence);
  return {
    id: (e.purpose ?? "blank") as AgentTemplate["id"],
    label,
    nameSuggestion: e.name,
    instructions: e.instructions,
    ...(schedule && schedule.kind === "schedule" && "cron" in schedule.cadence ? { schedule: { cron: schedule.cadence.cron, prompt: schedule.prompt } } : {}),
  };
}

// ---- reading -------------------------------------------------------------------------------------------------

/** One sentence naming what is wrong with a zip that is not an agent export. */
export class ExportError extends Error {}

const dec = new TextDecoder("utf-8", { fatal: true });

function text(entry: ZipEntry): string {
  try {
    return dec.decode(entry.data);
  } catch {
    throw new ExportError(`“${entry.name}” is not text.`);
  }
}

function split(src: string, file: string): { front: Record<string, YamlValue>; rest: string } {
  const m = /^---\r?\n([\s\S]*?)\r?\n---\r?\n?([\s\S]*)$/.exec(src);
  if (!m) throw new ExportError(`“${file}” has no front matter (--- … ---).`);
  try {
    const front = parseYaml(m[1]!);
    if (front === null || typeof front !== "object" || Array.isArray(front)) throw new ExportError(`“${file}” has no front matter.`);
    return { front, rest: m[2]! };
  } catch (e) {
    if (e instanceof YamlError) throw new ExportError(`“${file}”: ${e.message}`);
    throw e;
  }
}

function str(v: YamlValue | undefined, what: string, file: string): string {
  if (typeof v !== "string" || v === "") throw new ExportError(`“${file}” needs ${what}.`);
  return v;
}

function yamlFile(entry: ZipEntry): Record<string, YamlValue> {
  try {
    const v = parseYaml(text(entry));
    if (v === null || typeof v !== "object" || Array.isArray(v)) throw new ExportError(`“${entry.name}” is not a map of settings.`);
    return v;
  } catch (e) {
    if (e instanceof YamlError) throw new ExportError(`“${entry.name}”: ${e.message}`);
    throw e;
  }
}

const KEY_RE = /^[a-z0-9]+(-[a-z0-9]+)*$/;

/**
 * The files of a zip as an export. Strict: a file name outside the known folders is ignored, but a known file that does
 * not read is refused with one sentence naming it. `entries` come from `readZip` (names already validated).
 */
export function parseExport(entries: ZipEntry[]): AgentExport {
  if (entries.length === 0) throw new ExportError("The zip is empty.");
  for (const e of entries) {
    const problem = nameProblem(e.name);
    if (problem) throw new ExportError(`Refused: ${problem}.`);
  }
  const roots = new Set(entries.map((e) => e.name.split("/")[0]!));
  if (roots.size !== 1 || entries.some((e) => !e.name.includes("/"))) throw new ExportError("This zip is not an Aurora agent export (it has no single agent folder).");
  const root = [...roots][0]!;
  const rel = (e: ZipEntry) => e.name.slice(root.length + 1);
  const byPath = new Map(entries.map((e) => [rel(e), e]));
  const agent = byPath.get("agent.yaml");
  if (!agent) throw new ExportError("This zip is not an Aurora agent export (agent.yaml is missing).");
  const a = yamlFile(agent);
  if (a.format !== EXPORT_FORMAT) throw new ExportError(typeof a.format === "number" && a.format > EXPORT_FORMAT ? "This export is from a newer Aurora than this one." : "This export has no format this page can read.");
  // A blank template has no name to suggest: an empty one is allowed.
  if (typeof a.name !== "string") throw new ExportError("“agent.yaml” needs a name.");
  const name = a.name;
  const model = a.model === undefined ? "default" : str(a.model, "a model name or default", "agent.yaml");
  if (a.may_message_first !== undefined && typeof a.may_message_first !== "boolean") throw new ExportError("“agent.yaml”: may_message_first is true or false.");
  const instructionsEntry = byPath.get("instructions.md");
  // An export carries at most one avatar; its size is checked here, its image by the server on import.
  const avatarFiles = [...byPath].filter(([p]) => AVATAR_FILE_RE.test(p));
  if (avatarFiles.length > 1) throw new ExportError("This export has more than one avatar (avatar.png, avatar.jpg, avatar.webp): keep one.");
  const avatarFile = avatarFiles[0];
  if (avatarFile && avatarFile[1].data.length > EXPORT_AVATAR_MAX) throw new ExportError(`“${avatarFile[1].name}” is larger than 512 KiB.`);
  const avatar: ExportAvatar | undefined = avatarFile ? { ext: AVATAR_FILE_RE.exec(avatarFile[0])![1] as AvatarExt, bytes: avatarFile[1].data } : undefined;

  const memories: ExportMemory[] = [];
  const skills = new Map<string, ExportSkill>();
  const automations: ExportAutomation[] = [];
  for (const [path, entry] of byPath) {
    if (/^memory\/[^/]+\.md$/.test(path)) {
      const { front, rest } = split(text(entry), entry.name);
      const key = str(front.key, "a key", entry.name);
      if (!KEY_RE.test(key) || key.length > 64) throw new ExportError(`“${entry.name}”: the key “${key.slice(0, 40)}” is not a kebab-case key of at most 64 characters.`);
      // (P6-MEMLOCK, C-1150) `locked` is a boolean; anything else refuses the file, naming it.
      if (front.locked !== undefined && typeof front.locked !== "boolean") throw new ExportError(`“${entry.name}”: locked is true or false.`);
      memories.push({
        key,
        updated: typeof front.updated === "string" ? front.updated : "",
        content: rest,
        ...(front.locked === true ? { locked: true } : {}),
      });
    } else if (/^skills\/[^/]+\/SKILL\.md$/.test(path)) {
      const { front, rest } = split(text(entry), entry.name);
      const skillName = str(front.name, "a name", entry.name);
      if (!KEY_RE.test(skillName) || skillName.length > 64) throw new ExportError(`“${entry.name}”: the skill name “${skillName.slice(0, 40)}” is not a kebab-case name of at most 64 characters.`);
      // (C-1173) An optional display name; the server checks its length and form.
      if (front.title !== undefined && typeof front.title !== "string") throw new ExportError(`“${entry.name}”: title is a line of text.`);
      const title = typeof front.title === "string" && front.title !== "" ? { title: front.title } : {};
      skills.set(skillName, { name: skillName, ...title, useItWhen: str(front["use-it-when"], "use-it-when", entry.name), body: rest, files: skills.get(skillName)?.files ?? [] });
    } else if (/^automations\/[^/]+\.yaml$/.test(path)) {
      automations.push(automation(yamlFile(entry), entry.name));
    }
  }
  for (const [path, entry] of byPath) {
    const m = /^skills\/([^/]+)\/(.+)$/.exec(path);
    if (m && m[2] !== "SKILL.md") skills.get(m[1]!)?.files.push({ path: m[2]!, content: entry.data });
  }
  return {
    format: EXPORT_FORMAT,
    name,
    ...(typeof a.purpose === "string" ? { purpose: a.purpose } : {}),
    model,
    may_message_first: a.may_message_first === true,
    instructions: instructionsEntry ? text(instructionsEntry) : "",
    memories: memories.sort((x, y) => x.key.localeCompare(y.key)),
    skills: [...skills.values()].sort((x, y) => x.name.localeCompare(y.name)),
    automations,
    ...(avatar ? { avatar } : {}),
  };
}

function automation(v: Record<string, YamlValue>, file: string): ExportAutomation {
  const name = str(v.name, "a name", file);
  const enabled = v.enabled === true;
  if (v.kind === "schedule") {
    const cadence = v.cadence;
    const map = cadence !== null && typeof cadence === "object" && !Array.isArray(cadence) ? cadence : undefined;
    const time_zone = typeof v.time_zone === "string" ? v.time_zone : "UTC";
    const prompt = str(v.prompt, "a prompt", file);
    if (map?.preset === "once") {
      const runAt = str(map.run_at, "a cadence with a run_at", file);
      if (!RUN_AT_RE.test(runAt)) throw new ExportError(`“${file}”: run_at is a local time like 2026-12-01T09:00 (read in time_zone).`);
      return { kind: "schedule", name, prompt, cadence: { preset: "once", run_at: runAt }, time_zone, enabled };
    }
    return { kind: "schedule", name, prompt, cadence: { cron: str(map?.cron, "a cadence with a cron", file) }, time_zone, enabled };
  }
  if (v.kind === "webhook") {
    const verification = v.verification;
    if (verification !== "signed" && verification !== "password" && verification !== "none") throw new ExportError(`“${file}”: verification is signed, password or none.`);
    return { kind: "webhook", name, instructions: typeof v.instructions === "string" ? v.instructions : "", verification, enabled };
  }
  throw new ExportError(`“${file}”: kind is schedule or webhook.`);
}

export { ZipError };
