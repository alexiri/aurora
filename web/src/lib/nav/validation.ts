export interface NameSlugValues {
  name: string;
  slug: string;
}

const SLUG_RE = /^[a-z0-9][a-z0-9-]{1,47}$/;

/** Client-side mirror of backend rules (UX only; the API stays authoritative). */
export function validateNameSlug({ name, slug }: NameSlugValues): Record<string, string> {
  const errs: Record<string, string> = {};
  const n = [...name.trim()].length;
  if (n === 0) errs.name = "Name is required.";
  else if (n > 80) errs.name = "Name must be at most 80 characters.";
  const s = slug.trim();
  if (s && !SLUG_RE.test(s)) {
    errs.slug = "Use 2 to 48 lowercase letters, digits or dashes, starting with a letter or digit.";
  }
  return errs;
}
