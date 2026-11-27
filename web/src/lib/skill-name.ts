/**
 * (P5-REFINE-12, behaviour-2; D-498) A person names a skill in words ("Weekly report"); that is its **title**, and
 * Aurora makes the folder name from it: kebab-case ASCII, at most 64 characters, never starting with the reserved
 * `aurora-`, unique by a numeric suffix. Everywhere but the editor's folder line a skill is called by its display name.
 */
import { SKILL_NAME_MAX } from "@/lib/api/skills";
import { slugify } from "@/lib/memory-key";

export const SKILL_RESERVED_PREFIX = "aurora-";
export const SKILL_FALLBACK_NAME = "skill";

/** The folder name for `display`, before any suffix. */
export function baseSkillName(display: string): string {
  let s = slugify(display).slice(0, SKILL_NAME_MAX).replace(/-+$/, "");
  if (!s) return SKILL_FALLBACK_NAME;
  // `aurora-*` belongs to the platform: a skill named "Aurora tips" becomes "my-aurora-tips".
  if (s.startsWith(SKILL_RESERVED_PREFIX) || s === "aurora") s = `my-${s}`.slice(0, SKILL_NAME_MAX).replace(/-+$/, "");
  return s;
}

/** `base` with `-n` (n ≥ 2), the base cut so the whole name stays within 64 characters. */
export function skillNameWithSuffix(base: string, n: number): string {
  if (n < 2) return base;
  const suffix = `-${n}`;
  return `${base.slice(0, SKILL_NAME_MAX - suffix.length).replace(/-+$/, "")}${suffix}`;
}

/** A folder name that is not in `taken`; `skip` > 0 moves past that many free candidates (a retry after a 409). */
export function newSkillName(display: string, taken: Iterable<string>, skip = 0): string {
  const used = new Set(taken);
  const base = baseSkillName(display);
  let n = 1;
  let free = 0;
  for (;;) {
    const name = skillNameWithSuffix(base, n);
    if (!used.has(name)) {
      if (free >= skip) return name;
      free += 1;
    }
    n += 1;
  }
}

/**
 * (D-498(b)) What a skill is called: its title, else its folder name made readable. Dashes become spaces and the first
 * letter is capitalized ("internal-comms" → "Internal comms"); a one-word name stays as written ("pdf").
 */
export function skillDisplayName(name: string, title?: string | null): string {
  if (title?.trim()) return title.trim();
  if (!name.includes("-")) return name;
  const s = name.replace(/-+/g, " ").trim();
  return s ? s.charAt(0).toUpperCase() + s.slice(1) : name;
}

/** A description in a list: as written, with only its first letter capitalized. */
export function descriptionSentence(description: string): string {
  const d = description.trim();
  return d ? d.charAt(0).toUpperCase() + d.slice(1) : d;
}

/** Openings that do not complete "Use it to …" (a sentence of its own, or the old "Use it when …" wording). */
const OWN_SENTENCE = /^(use|used|when|whenever|if|for|someone|asked|this|these|a|an|the|any|anything)\b/i;

/**
 * (D-498(c)) The panel's "Use it to …" line for a skill written in Aurora, whose description completes that sentence.
 * A description that is a sentence of its own stays as written; so does repository or app text (`asWritten`).
 */
export function useItToLine(description: string, asWritten = false): string {
  const d = description.trim();
  if (!d || asWritten || OWN_SENTENCE.test(d)) return descriptionSentence(d);
  // "PR summaries" keeps its capitals.
  const rest = /^\p{Lu}\p{Ll}/u.test(d) ? d.charAt(0).toLowerCase() + d.slice(1) : d;
  return `Use it to ${rest}`;
}
