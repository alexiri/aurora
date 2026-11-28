/**
 * (D-508(h), C-1178) The back line of a place (Manage <org>, Administration, Your account): "the workspace and page you
 * came from". While someone is in a workspace, the page they are on is kept in this tab's session storage — never in a
 * URL. A place reached with no record (a link, a new tab) falls back to their last workspace.
 */
import { useEffect, useMemo, useSyncExternalStore } from "react";

import { readRecent, type ScopeMembership } from "@/lib/nav/recent-scopes";
import { wsRoutes } from "@/lib/routes";
import { workspaceDisplayName } from "@/lib/workspace-name";

const KEY = "aurora.place-return";

export interface PlaceRecord {
  /** The signed-in person: another account in the same tab never reads it. */
  userId: string;
  orgSlug: string;
  wsSlug: string;
  wsName: string;
  /** The page, with its query: always inside that workspace. */
  path: string;
  /** An emergency reader's workspace is not among their memberships (FIX-277). */
  reader: boolean;
}

/** An internal path only: starts with one "/", no scheme, no protocol-relative "//", no backslash. */
export function isInternalPath(href: string): boolean {
  return href.startsWith("/") && !href.startsWith("//") && !href.includes("\\") && !/^[a-z][a-z0-9+.-]*:/i.test(href);
}

/** Is `path` (with an optional query or hash) the workspace's own address or a page under it, with no dot segments? */
export function isWorkspacePath(path: string, orgSlug: string, wsSlug: string): boolean {
  if (!isInternalPath(path)) return false;
  const base = wsRoutes(orgSlug, wsSlug).base;
  const pathname = path.split(/[?#]/)[0]!;
  if (pathname !== base && !pathname.startsWith(`${base}/`)) return false;
  return !pathname.split("/").some((s) => /^(\.|%2e){1,2}$/i.test(s));
}

export function parsePlaceRecord(raw: string | null): PlaceRecord | null {
  try {
    const v: unknown = raw ? JSON.parse(raw) : null;
    if (!v || typeof v !== "object") return null;
    const { userId, orgSlug, wsSlug, wsName, path, reader } = v as Record<string, unknown>;
    if (typeof userId !== "string" || typeof orgSlug !== "string" || typeof wsSlug !== "string" || typeof wsName !== "string" || typeof path !== "string") return null;
    if (!isWorkspacePath(path, orgSlug, wsSlug)) return null;
    return { userId, orgSlug, wsSlug, wsName, path, reader: reader === true };
  } catch {
    return null;
  }
}

const noopSubscribe = () => () => {};
function rawRecord(): string | null {
  try {
    return window.sessionStorage.getItem(KEY);
  } catch {
    // Storage is blocked: the fallback decides.
    return null;
  }
}

/** The record, read from session storage (null on the server, so the first render matches). */
export function usePlaceRecord(): PlaceRecord | null {
  const raw = useSyncExternalStore(noopSubscribe, rawRecord, () => null);
  return useMemo(() => parsePlaceRecord(raw), [raw]);
}

export function writePlaceRecord(record: PlaceRecord): void {
  try {
    window.sessionStorage.setItem(KEY, JSON.stringify(record));
  } catch {
    // Storage is blocked: nothing is remembered.
  }
}

export interface PlaceTarget {
  orgSlug: string;
  wsSlug: string;
  wsName: string;
}

/**
 * The workspace a person is sent to when nothing else says which: the one they used last, else their personal one, else
 * the first. With `orgSlug`, only that organization's. `undefined` when they have none.
 */
export function lastWorkspace(userId: string | undefined, memberships: ScopeMembership[] | undefined, orgSlug?: string): PlaceTarget | undefined {
  const list = (memberships ?? [])
    .filter((m) => (m.status === undefined || m.status === "active") && (orgSlug === undefined || m.org.slug === orgSlug))
    .flatMap((m) => m.workspaces.map((w) => ({ orgSlug: m.org.slug, w })));
  const recent = readRecent(userId);
  const hit =
    recent.map((key) => list.find((x) => `${x.orgSlug}/${x.w.slug}` === key)).find((x) => x !== undefined) ??
    list.find((x) => x.w.personal === true) ??
    list[0];
  if (!hit) return undefined;
  return { orgSlug: hit.orgSlug, wsSlug: hit.w.slug, wsName: workspaceDisplayName({ name: hit.w.name ?? hit.w.slug, personal: hit.w.personal }, null) };
}

export interface BackLink {
  href: string;
  name: string;
}

/**
 * Where a place's back line goes: the recorded page, while it is still this person's to open; else their last workspace
 * (in `orgSlug` for Manage <org>); else nowhere, and the line is hidden. Nothing while the session loads.
 */
export function backLink(
  record: PlaceRecord | null,
  userId: string | undefined,
  memberships: ScopeMembership[] | undefined,
  orgSlug?: string,
): BackLink | undefined {
  if (!userId || !memberships) return undefined;
  if (record && record.userId === userId) {
    const known = memberships.some((m) => m.org.slug === record.orgSlug && m.workspaces.some((w) => w.slug === record.wsSlug));
    if (known || record.reader) return { href: record.path, name: record.wsName };
  }
  const w = lastWorkspace(userId, memberships, orgSlug);
  return w ? { href: wsRoutes(w.orgSlug, w.wsSlug).home(), name: w.wsName } : undefined;
}

/**
 * Keeps the record up to date while `place` is the workspace on screen (`null` elsewhere). The address is read again on
 * every click and key press, before it navigates anywhere: a tab changed in the query is part of "the page".
 */
export function useRecordPlace(place: Omit<PlaceRecord, "path"> | null, pathname: string): void {
  const { userId, orgSlug, wsSlug, wsName, reader } = place ?? {};
  useEffect(() => {
    if (!userId || !orgSlug || !wsSlug || !wsName) return;
    const here = () => `${window.location.pathname}${window.location.search}`;
    const write = (path: string) => {
      if (isWorkspacePath(path, orgSlug, wsSlug)) writePlaceRecord({ userId, orgSlug, wsSlug, wsName, reader: reader === true, path });
    };
    write(isWorkspacePath(here(), orgSlug, wsSlug) ? here() : pathname);
    const capture = () => write(here());
    const events = ["click", "keydown", "pagehide"] as const;
    for (const e of events) window.addEventListener(e, capture, true);
    return () => {
      for (const e of events) window.removeEventListener(e, capture, true);
    };
  }, [userId, orgSlug, wsSlug, wsName, reader, pathname]);
}
