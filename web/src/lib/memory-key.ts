/**
 * (P5-REFINE-12, behaviour-1, rec 5) A person writes a memory in one box; Aurora makes the key. The key is a kebab-case
 * ASCII slug of at most 64 characters (`MEMORY_KEY_RE`), taken from the title when there is one, else from the first
 * words of the text, and made unique against the agent's keys by a numeric suffix ("bobs-tea", "bobs-tea-2").
 */
import { MEMORY_KEY_MAX } from "@/lib/api/memory";

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

/** The name a memory goes by in lists: its title, else the first line of the text, 60 characters. */
export function memoryName(m: { title?: string | null; content?: string | null; index_line?: string | null }): string {
  const t = m.title?.trim();
  if (t) return cut(t, 60);
  const first = (m.content ?? "").split("\n").find((l) => l.trim() !== "");
  if (first) return cut(first.replace(/^#+\s*/, "").trim(), 60);
  return cut((m.index_line ?? "").trim() || "Untitled", 60);
}

/** A one-line preview for a list: the description, else the text after the first line when a title leads, else the text. */
export function memoryPreview(m: { title?: string | null; description?: string | null; content?: string | null }): string {
  const d = m.description?.trim();
  if (d) return cut(d.split("\n")[0]!, 90);
  const lines = (m.content ?? "").split("\n").map((l) => l.trim()).filter(Boolean);
  const line = m.title?.trim() ? lines[0] : (lines[1] ?? "");
  return cut((line ?? "").replace(/^#+\s*/, ""), 90);
}

function cut(s: string, max: number): string {
  return s.length <= max ? s : `${s.slice(0, max - 1).trimEnd()}…`;
}
