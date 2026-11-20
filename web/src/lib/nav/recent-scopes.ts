/**
 * The workspaces a person visited most recently, newest first, as `org-slug/ws-slug`, kept per person in this browser
 * (D-389: the switcher orders its lists by it). P5-REFINE-2: the same list gives "the last scope" to the pages that have
 * none of their own (Account, a 404) — no new storage key.
 */
const recentKey = (userId: string | undefined) => `aurora.recent-scopes:${userId ?? "anon"}`;

export function readRecent(userId: string | undefined): string[] {
  try {
    const raw = window.localStorage.getItem(recentKey(userId));
    const parsed: unknown = raw ? JSON.parse(raw) : [];
    return Array.isArray(parsed) ? parsed.filter((x): x is string => typeof x === "string") : [];
  } catch {
    return [];
  }
}

export function writeRecent(userId: string | undefined, key: string) {
  try {
    const next = [key, ...readRecent(userId).filter((k) => k !== key)].slice(0, 50);
    window.localStorage.setItem(recentKey(userId), JSON.stringify(next));
  } catch {
    // Storage is blocked or full: the list keeps its default order.
  }
}

export interface ScopeMembership {
  org: { slug: string; name?: string | null };
  status?: string;
  workspaces: { slug: string; name?: string | null }[];
}

export interface LastScope {
  orgSlug: string;
  orgName: string;
  wsSlug?: string;
  wsName?: string;
}

/**
 * The newest recent workspace that still exists in the person's memberships; else the first (active) organization they
 * belong to. `undefined` when they belong to none.
 */
export function lastScope(userId: string | undefined, memberships: ScopeMembership[] | undefined): LastScope | undefined {
  const active = (memberships ?? []).filter((m) => m.status === undefined || m.status === "active");
  for (const key of readRecent(userId)) {
    const [orgSlug, wsSlug] = key.split("/");
    const m = active.find((x) => x.org.slug === orgSlug);
    const w = m?.workspaces.find((x) => x.slug === wsSlug);
    if (m && w) return { orgSlug: m.org.slug, orgName: m.org.name ?? m.org.slug, wsSlug: w.slug, wsName: w.name ?? w.slug };
  }
  const first = active[0];
  return first ? { orgSlug: first.org.slug, orgName: first.org.name ?? first.org.slug } : undefined;
}
