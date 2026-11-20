/**
 * (P5-REFINE-12, behaviour-2) A person names a skill in words ("Weekly report"); Aurora makes the skill's folder name:
 * kebab-case ASCII, at most 64 characters, never starting with the reserved `aurora-`, unique by a numeric suffix. The
 * display name is the stored name with hyphens as spaces and a capital first letter.
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

/** "weekly-report" → "Weekly report". */
export function skillDisplayName(name: string): string {
  const s = name.replace(/-+/g, " ").trim();
  return s ? s.charAt(0).toUpperCase() + s.slice(1) : name;
}
