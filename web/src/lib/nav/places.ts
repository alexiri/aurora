/**
 * (D-508(f), P5-NAV-3) The places a person can enter, as the picker, the rail's foot and the phone's top bar list them.
 * A place comes only from what this person may read: their session's memberships, the open organization's workspace list
 * (only the rows that are their own), and, while their own emergency grant is open, the workspace on screen.
 */
import { isEmergencyReader } from "@/lib/emergency";
import { lastWorkspace } from "@/lib/nav/manage-return";
import type { ScopeMembership } from "@/lib/nav/recent-scopes";
import { orgRoutes, wsRoutes } from "@/lib/routes";
import { personName, workspaceDisplayName } from "@/lib/workspace-name";

/** How many of an organization's workspaces the picker lists before "All n workspaces". */
export const RECENT_LIMIT = 5;
/** How many other organizations the picker lists before "All n organizations". */
export const ORG_LIMIT = 3;
/** The search field appears when there are more places than this. */
export const SEARCH_ABOVE = 6;

export type OrgRole = "owner" | "admin" | "member";

export interface PlaceWorkspace {
  slug: string;
  /** As this person reads it ("Your workspace"). */
  label: string;
  /** The stored name, also searched. */
  stored: string;
  /** Whose initial the mark shows: a personal workspace's owner, else the workspace. */
  mark: string;
  /** "Owner", "Approver", "Member", or "Emergency read access". */
  role: string;
  personal: boolean;
}

export interface PlaceOrg {
  id?: string;
  slug: string;
  name: string;
  /** The person's role in the organization, when they have one. */
  role?: OrgRole;
  /** They may open Manage <org>. */
  manage: boolean;
  /** Their workspaces, the ones used last first. */
  workspaces: PlaceWorkspace[];
  /** How many workspaces the organization has, for someone who may list them all. */
  total?: number;
}

interface Membership {
  org: { id?: string; slug: string; name?: string };
  role?: string;
  effective_role?: string | null;
  status?: string;
  workspaces: { id?: string; slug: string; name?: string; my_role?: string; personal?: boolean }[];
}

interface ListedWorkspace {
  slug: string;
  name: string;
  my_role?: string | null;
  personal_of?: { id: string; name: string } | null;
}

export interface PlacesInput {
  viewer: { id?: string; display_name: string } | undefined;
  instanceAdmin: boolean;
  memberships: Membership[];
  /** The organization whose own list was read (the one the picker is about). */
  open?: { id: string; slug: string; name: string } | null;
  openList?: ListedWorkspace[];
  /** The workspace on screen, as its own answer says it (FIX-277). */
  reading?: { slug: string; name: string; my_role?: string | null; emergency_reader?: boolean; personal_of?: { id: string; name: string } | null } | null;
  /** `org/ws` keys, newest first. */
  recent: string[];
}

export const capitalised = (r: string) => (r ? r.charAt(0).toUpperCase() + r.slice(1) : "");

export function buildPlaces(input: PlacesInput): PlaceOrg[] {
  const { viewer, instanceAdmin, memberships, open, openList, reading, recent } = input;
  const toPlace = (w: { slug: string; name: string; personal?: boolean; personal_of?: { id: string; name: string } | null }, role: string): PlaceWorkspace => {
    const label = workspaceDisplayName(w, viewer);
    const own = w.personal === true || (!!w.personal_of && w.personal_of.id === viewer?.id);
    // (P5-POLISH-8, D4) A personal workspace's mark is its owner's initial ("B"), not the "Y" of "Your workspace".
    const mark = w.personal_of ? personName(w.personal_of.name) : own && viewer?.display_name ? personName(viewer.display_name) : label;
    return { slug: w.slug, label, stored: w.name, mark, role, personal: own };
  };
  const orgs: PlaceOrg[] = memberships.map((m) => {
    const active = m.status === undefined || m.status === "active";
    const role = (m.effective_role ?? m.role) as OrgRole | undefined;
    return {
      id: m.org.id,
      slug: m.org.slug,
      name: m.org.name ?? m.org.slug,
      role,
      manage: instanceAdmin || (active && (role === "owner" || role === "admin")),
      // (FIX-235) Only workspaces this person is in (`my_role` set).
      workspaces: m.workspaces.flatMap((w) => (w.my_role ? [toPlace({ slug: w.slug, name: w.name ?? w.slug, personal: w.personal }, capitalised(w.my_role))] : [])),
    };
  });
  if (open) {
    let at = orgs.findIndex((o) => o.slug === open.slug);
    if (at < 0) {
      // An instance admin in an organization they don't belong to: it is listed while they are there.
      at = orgs.push({ id: open.id, slug: open.slug, name: open.name, manage: instanceAdmin, workspaces: [] }) - 1;
    }
    const o = orgs[at]!;
    if (openList) {
      // The organization's own list is newer than the session's. An administrator's also holds every workspace
      // (C-1160): only their own rows become places.
      const fresh = openList.flatMap((w) => {
        if (!w.my_role) return [];
        const personal = o.workspaces.some((x) => x.slug === w.slug && x.personal);
        return [toPlace({ ...w, personal }, capitalised(w.my_role))];
      });
      orgs[at] = { ...o, workspaces: fresh, total: o.manage ? openList.length : undefined };
    }
    // (FIX-277, D-478) An emergency reader's workspace is a place only while their own grant is open.
    if (reading && isEmergencyReader(reading) && !orgs[at]!.workspaces.some((w) => w.slug === reading.slug)) {
      orgs[at] = { ...orgs[at]!, workspaces: [...orgs[at]!.workspaces, toPlace(reading, "Emergency read access")] };
    }
  }
  const rank = (key: string) => {
    const i = recent.indexOf(key);
    return i < 0 ? Number.MAX_SAFE_INTEGER : i;
  };
  return orgs.map((o) => ({
    ...o,
    workspaces: [...o.workspaces].sort(
      (a, b) => rank(`${o.slug}/${a.slug}`) - rank(`${o.slug}/${b.slug}`) || Number(b.personal) - Number(a.personal) || a.label.localeCompare(b.label),
    ),
  }));
}

/** The place an address is (D-508(a)); the manage page of a workspace (`/o/<org>/workspaces/<ws>`) is Manage <org>. */
export function placeOf(pathname: string, orgSlug: string | undefined, wsSlug: string | undefined, instanceAdmin: boolean) {
  if (pathname.startsWith("/admin") && instanceAdmin) return { kind: "admin" as const };
  if (pathname === "/account" || pathname.startsWith("/account/")) return { kind: "account" as const };
  if (orgSlug && !wsSlug && !pathname.startsWith("/admin")) return { kind: "org" as const, orgSlug };
  return { kind: "workspace" as const, orgSlug, wsSlug };
}

/** A person can get into an organization through a workspace of theirs, or through Manage <org>. */
export const enterable = (o: PlaceOrg) => o.manage || o.workspaces.length > 0;

/** The other organizations a person can enter, the ones visited last first. */
export function otherOrgs(orgs: PlaceOrg[], current: string | undefined, recent: string[]): PlaceOrg[] {
  const rank = (o: PlaceOrg) => {
    const i = recent.findIndex((k) => k.startsWith(`${o.slug}/`));
    return i < 0 ? Number.MAX_SAFE_INTEGER : i;
  };
  return orgs.filter((o) => o.slug !== current && enterable(o)).sort((a, b) => rank(a) - rank(b) || a.name.localeCompare(b.name));
}

/** Choosing another organization: your last workspace there; with none, its Manage when you may manage it. */
export function orgTarget(o: PlaceOrg, userId: string | undefined, memberships: ScopeMembership[] | undefined): string | undefined {
  const last = lastWorkspace(userId, memberships, o.slug);
  const slug = last?.wsSlug ?? o.workspaces[0]?.slug;
  if (slug) return wsRoutes(o.slug, slug).home();
  return o.manage ? orgRoutes(o.slug).home() : undefined;
}

/** Every place counted for "more than six": workspaces, Manage entries and Administration. */
export function placeCount(orgs: PlaceOrg[], instanceAdmin: boolean): number {
  return orgs.reduce((n, o) => n + o.workspaces.length + (o.manage ? 1 : 0), 0) + (instanceAdmin ? 1 : 0);
}
