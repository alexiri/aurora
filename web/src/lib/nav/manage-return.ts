/**
 * (P5-REFINE-17, rec 8) The "Manage <org>" area keeps the rail of the workspace the person was in: remembered per
 * browser tab (session storage, like Administration's return link), else their personal workspace, else the first one.
 */
import { useMemo, useSyncExternalStore } from "react";

const KEY = "aurora.manage-rail";

export interface RailWorkspace {
  orgSlug: string;
  wsSlug: string;
  wsName: string;
}

const noopSubscribe = () => () => {};
function rawRail(): string | null {
  try {
    return window.sessionStorage.getItem(KEY);
  } catch {
    return null;
  }
}

/** The remembered workspace, read from session storage (null on the server, so the first render matches). */
export function useRailWorkspace(): RailWorkspace | null {
  const raw = useSyncExternalStore(noopSubscribe, rawRail, () => null);
  return useMemo(() => parseRail(raw), [raw]);
}

export function readRailWorkspace(): RailWorkspace | null {
  return parseRail(rawRail());
}

function parseRail(raw: string | null): RailWorkspace | null {
  try {
    const v: unknown = raw ? JSON.parse(raw) : null;
    if (v && typeof v === "object" && "orgSlug" in v && "wsSlug" in v && "wsName" in v) {
      const r = v as Record<string, unknown>;
      if (typeof r.orgSlug === "string" && typeof r.wsSlug === "string" && typeof r.wsName === "string") {
        return { orgSlug: r.orgSlug, wsSlug: r.wsSlug, wsName: r.wsName };
      }
    }
  } catch {
    // Storage is blocked: the fallbacks decide.
  }
  return null;
}

export function writeRailWorkspace(w: RailWorkspace): void {
  try {
    window.sessionStorage.setItem(KEY, JSON.stringify(w));
  } catch {
    // Storage is blocked: nothing is remembered.
  }
}

export interface RailMembership {
  org: { slug: string };
  workspaces: { slug: string; name?: string | null; personal?: boolean }[];
}

/** The workspace whose rail the Manage area shows: the remembered one in this organization, else the personal one, else the first. */
export function pickRailWorkspace(orgSlug: string, stored: RailWorkspace | null, memberships: RailMembership[] | undefined): { wsSlug: string; wsName: string } | null {
  const list = memberships?.find((m) => m.org.slug === orgSlug)?.workspaces ?? [];
  if (stored && stored.orgSlug === orgSlug && (list.length === 0 || list.some((w) => w.slug === stored.wsSlug))) {
    return { wsSlug: stored.wsSlug, wsName: stored.wsName };
  }
  const w = list.find((x) => x.personal === true) ?? list[0];
  return w ? { wsSlug: w.slug, wsName: w.name ?? w.slug } : null;
}
