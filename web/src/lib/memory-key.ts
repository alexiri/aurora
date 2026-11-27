/**
 * (P5-REFINE-12, behaviour-1, rec 5) A person writes a memory in one box; Aurora makes the key. The key is a kebab-case
 * ASCII slug of at most 64 characters (`MEMORY_KEY_RE`), taken from the title when there is one, else from the first
 * words of the text, and made unique against the agent's keys by a numeric suffix ("bobs-tea", "bobs-tea-2").
 */
import { MEMORY_KEY_MAX, MEMORY_KEY_RE } from "@/lib/api/memory";

/** How many words of the text name a memory that has no title. */
const FIRST_WORDS = 6;
/** The key when nothing usable is left (a title or text with no letters or digits we can keep). */
export const FALLBACK_KEY = "note";

/** `text` as a slug: diacritics dropped, apostrophes removed, anything else that is not a letter or digit becomes one hyphen. */
export function slugify(text: string): string {
  return text
    .normalize("NFKD")
    .replace(/[̀-ͯ]/g, "")
    .replace(/['’`]/g, "")
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "");
}

/** The words the key is made from: the title, else the first words of the text. */
function source(title: string, content: string): string {
  const t = title.trim();
  if (t) return t;
  return content.trim().split(/\s+/).slice(0, FIRST_WORDS).join(" ");
}

/** The key without its suffix: at most 64 characters, never ending in a hyphen; `note` when nothing is left. */
export function baseKey(title: string, content: string): string {
  const s = slugify(source(title, content));
  return s ? s.slice(0, MEMORY_KEY_MAX).replace(/-+$/, "") || FALLBACK_KEY : FALLBACK_KEY;
}

/** `base` with `-n` appended (n ≥ 2), the base cut so the whole key stays within 64 characters. */
export function withSuffix(base: string, n: number): string {
  if (n < 2) return base;
  const suffix = `-${n}`;
  return `${base.slice(0, MEMORY_KEY_MAX - suffix.length).replace(/-+$/, "")}${suffix}`;
}

/**
 * A key for a new memory that is not in `taken`. `skip` makes the next candidate after a refusal: the first attempt is
 * 0, a retry after a 409 passes 1, 2, … so it moves on to the next free suffix.
 */
export function newMemoryKey(title: string, content: string, taken: Iterable<string>, skip = 0): string {
  const used = new Set(taken);
  const base = baseKey(title, content);
  let n = 1;
  let free = 0;
  for (;;) {
    const key = withSuffix(base, n);
    if (!used.has(key)) {
      if (free >= skip) return key;
      free += 1;
    }
    n += 1;
  }
}

/** How much of one line is reduced to plain text: far more than a name shows, and a bound on the work. */
const NAME_LINE_MAX = 400;

/**
 * One line of Markdown as plain text, as search snippets are: no heading, quote, list or task-list marks, no emphasis or
 * code marks, a link or picture by its text. What stands inside code, escaped marks, underscores inside a word and marks
 * that pair with nothing stay. Empty when the line is only marks (a code fence, a rule, a table's rule).
 */
function plainLine(line: string): string {
  let s = line.replace(/\0/g, "").slice(0, NAME_LINE_MAX).trim();
  if (/^(```|~~~)/.test(s) || /^[-*_=|:\s]+$/.test(s)) return "";
  // Block marks, however they are nested ("> - ## Rota"). A task box counts only after its list mark.
  let before: string;
  do {
    before = s;
    s = s.replace(/^(?:>\s*|#{1,6}(?:\s+|$)|(?:[-*+]|\d{1,9}[.)])(?:\s+\[[ xX]\])?(?:\s+|$))/, "");
  } while (s !== before);
  // Code and escaped characters are set aside, so the rules below leave them alone.
  const kept: string[] = [];
  const keep = (text: string) => `\0${kept.push(text) - 1}\0`;
  s = s
    .replace(/(`+)(.+?)\1(?!`)/g, (_m, _ticks, code: string) => keep(code.trim()))
    .replace(/\\([!-/:-@[-`{-~])/g, (_m, ch: string) => keep(ch))
    .replace(/!?\[([^\]]*)\](?:\((?:[^()]|\([^()]*\))*\)|\[[^\]]*\])/g, "$1")
    .replace(/<((?:https?:\/\/|mailto:)[^\s>]+)>/g, (_m, url: string) => keep(url));
  do {
    before = s;
    s = s
      .replace(/(\*{1,3}|~~)(?=[^\s*~])(.*?[^\s*~])\1/g, "$2")
      .replace(/(?<![\p{L}\p{N}_])(_{1,3})(?=[^\s_])(.*?[^\s_])\1(?![\p{L}\p{N}_])/gu, "$2");
  } while (s !== before);
  return s.replace(/\0(\d+)\0/g, (_m, i: string) => kept[Number(i)] ?? "").trim();
}

/** The name a memory goes by in lists: its title, else the first line of the text as plain text, 60 characters. */
export function memoryName(m: { title?: string | null; content?: string | null; index_line?: string | null }): string {
  const t = m.title?.trim();
  if (t) return cut(t, 60);
  for (const line of (m.content ?? "").split("\n")) {
    const plain = plainLine(line);
    if (plain) return cut(plain, 60);
  }
  return cut((m.index_line ?? "").trim() || "Untitled", 60);
}

/** Keys the server keeps for itself. */
const RESERVED_KEYS = ["index", "search"];

/**
 * What is wrong with a key the person typed, else null: the server's rule (lowercase letters and digits joined by single
 * dashes, at most 64 characters, not a reserved word), said before the request. The server stays authoritative.
 */
export function keyProblem(key: string): string | null {
  if (!key) return "Give the memory a key.";
  if (key.length > MEMORY_KEY_MAX || !MEMORY_KEY_RE.test(key)) return `Use lowercase words joined by dashes, at most ${MEMORY_KEY_MAX} characters.`;
  if (RESERVED_KEYS.includes(key)) return `“${key}” is reserved. Choose another key.`;
  return null;
}

export function cut(s: string, max: number): string {
  return s.length <= max ? s : `${s.slice(0, max - 1).trimEnd()}…`;
}
