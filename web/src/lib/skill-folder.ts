/**
 * (P5-SKILLS-3, skill-4) **Upload a skill folder**: reads a picked folder into the skill it would create, and says in
 * plain words, per file, everything that stops it, before anything is written. The server checks again.
 */
import {
  SKILL_BODY_MAX,
  SKILL_DESCRIPTION_MAX,
  SKILL_FILES_MAX,
  SKILL_TITLE_MAX,
  SKILL_TOTAL_MAX,
  decodeSkillText,
  skillContentSize,
  validateSkillName,
  validateSkillPath,
  type NewSkillFile,
} from "@/lib/api/skills";
import { formatCount } from "@/lib/format-date";
import { baseSkillName, skillDisplayName } from "@/lib/skill-name";

/** One picked file: its path from the picked folder's parent (`folder/scripts/run.sh`). */
export interface PickedFile {
  path: string;
  size: number;
  read: () => Promise<Uint8Array>;
}

export interface SkillFolder {
  /** The folder name the skill will have. */
  name: string;
  title: string | null;
  description: string;
  body: string;
  files: (NewSkillFile & { size: number })[];
  /** The instructions plus the files, as the budget counts them. */
  size: number;
  /** Files the computer adds on its own, left out. */
  skipped: string[];
}

export type FolderResult = { ok: true; folder: SkillFolder } | { ok: false; problems: string[] };

const SYSTEM_FILE = /(^|\/)(\.DS_Store|Thumbs\.db|desktop\.ini|__MACOSX|\.git)(\/|$)/;
/** Nothing this big can be part of a skill; it is not read. */
const READ_MAX = SKILL_TOTAL_MAX * 4;
const FIELDS = new Set(["name", "title", "description"]);

function unquote(value: string): string {
  const v = value.trim();
  if (v.length > 1 && v.startsWith('"') && v.endsWith('"')) {
    try {
      return JSON.parse(v) as string;
    } catch {
      return v.slice(1, -1);
    }
  }
  if (v.length > 1 && v.startsWith("'") && v.endsWith("'")) return v.slice(1, -1).replaceAll("''", "'");
  return v;
}

/**
 * The `name`, `title` and `description` of a `SKILL.md`'s front matter, and the text after it. Only those three
 * top-level keys are read, on one line or folded over indented ones (`>`, `|`, a long plain value); other keys are
 * left alone. Null without a front matter block.
 */
export function parseSkillMd(source: string): { fields: Record<string, string>; body: string } | null {
  const m = /^﻿?---\r?\n([\s\S]*?)\r?\n---[ \t]*(?:\r?\n|$)([\s\S]*)$/.exec(source);
  if (!m) return null;
  const lines = m[1]!.split(/\r?\n/);
  const fields: Record<string, string> = {};
  for (let i = 0; i < lines.length; i++) {
    const head = /^([A-Za-z_][A-Za-z0-9_-]*):(.*)$/.exec(lines[i]!);
    if (!head || !FIELDS.has(head[1]!)) continue;
    const first = head[2]!.trim();
    const more: string[] = [];
    while (i + 1 < lines.length && (/^\s+\S/.test(lines[i + 1]!) || lines[i + 1]!.trim() === "")) more.push(lines[++i]!.trim());
    const block = /^[>|][+-]?$/.test(first);
    const joined = [...(block ? [] : [first]), ...more].filter(Boolean).join(" ");
    fields[head[1]!] = unquote(joined);
  }
  return { fields, body: m[2]!.replace(/^(\r?\n)+/, "") };
}

export async function readSkillFolder(picked: PickedFile[], taken: Iterable<string>, owner: string): Promise<FolderResult> {
  const problems: string[] = [];
  const skipped: string[] = [];
  const rootName = picked[0]?.path.split("/")[0] ?? "";
  const inside = picked
    .map((f) => ({ ...f, rel: f.path.split("/").slice(1).join("/") }))
    .filter((f) => {
      if (!SYSTEM_FILE.test(f.rel)) return true;
      skipped.push(f.rel);
      return false;
    })
    .sort((a, b) => a.rel.localeCompare(b.rel));
  const main = inside.find((f) => f.rel === "SKILL.md");
  if (!main) return { ok: false, problems: ["The folder has no SKILL.md at its top level."] };
  const others = inside.filter((f) => f !== main);

  let name = "";
  let title: string | null = null;
  let description = "";
  let body = "";
  const mainText = main.size > READ_MAX ? null : decodeSkillText(await main.read());
  const parsed = mainText === null ? null : parseSkillMd(mainText);
  if (mainText === null) problems.push(main.size > READ_MAX ? "SKILL.md: it is too large." : "SKILL.md: it isn’t text.");
  else if (!parsed) problems.push("SKILL.md: it needs a front matter block (between two --- lines) with a description.");
  else {
    const given = parsed.fields.name ?? "";
    name = [given, rootName].find((n) => validateSkillName(n) === null) ?? baseSkillName(given || rootName);
    title = parsed.fields.title?.trim() || null;
    description = (parsed.fields.description ?? "").trim();
    body = parsed.body;
    if (!description) problems.push("SKILL.md: its front matter needs a description.");
    else if ([...description].length > SKILL_DESCRIPTION_MAX) problems.push(`SKILL.md: the description is longer than ${SKILL_DESCRIPTION_MAX} characters.`);
    if (title && [...title].length > SKILL_TITLE_MAX) problems.push(`SKILL.md: the title is longer than ${SKILL_TITLE_MAX} characters.`);
    if ([...body].length > SKILL_BODY_MAX) problems.push(`SKILL.md: the instructions are longer than ${formatCount(SKILL_BODY_MAX)} characters.`);
    if (new Set(taken).has(name)) problems.push(`${owner} already has a skill in the folder “${name}” (${skillDisplayName(name)}). Remove it first, or rename the skill.`);
  }
  if (others.length > SKILL_FILES_MAX) problems.push(`A skill can have at most ${SKILL_FILES_MAX} files besides SKILL.md; this folder has ${others.length}.`);

  const files: SkillFolder["files"] = [];
  for (const f of others.slice(0, SKILL_FILES_MAX)) {
    const bad = validateSkillPath(f.rel);
    if (bad) {
      problems.push(`${f.rel}: ${bad}`);
      continue;
    }
    if (f.size > READ_MAX) {
      problems.push(`${f.rel}: it is too large.`);
      continue;
    }
    const content = await f.read();
    const text = decodeSkillText(content);
    // A browser does not tell whether a file may be run: a script says so by its first line.
    files.push({ path: f.rel, content, executable: !!text?.startsWith("#!"), size: skillContentSize(content) });
  }
  const size = [...body].length + files.reduce((n, f) => n + f.size, 0);
  if (size > SKILL_TOTAL_MAX) {
    problems.push(`The instructions and files are ${formatCount(size)} characters together; a skill can total at most ${formatCount(SKILL_TOTAL_MAX)}.`);
  }
  if (problems.length) return { ok: false, problems };
  return { ok: true, folder: { name, title, description, body, files, size, skipped } };
}
