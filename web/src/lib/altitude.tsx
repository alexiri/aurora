"use client";

import { QueryClientContext } from "@tanstack/react-query";
import { useParams, usePathname } from "next/navigation";
import { createContext, useCallback, useContext, useEffect, useId, useMemo, useState, type ReactNode } from "react";

import { useMe, useOrg, useResolve, useWorkspace, useWorkspaces } from "@/lib/api/queries";
import { altitudeAllows, viewerFloor, type Altitude } from "@/lib/api/roles";
import { useMoreControls } from "@/lib/api/use-more-controls";

/**
 * Where the viewer is: the org and workspace slugs and the path. Outside a Next route (a component rendered alone, or a
 * test that mocks `next/navigation` without these hooks) there is no place: the floor is then Use.
 */
function useRoutePlace(): { org?: string; ws?: string; pathname: string } {
  let params: { org?: string; ws?: string } | null = null;
  let pathname: string | null = null;
  try {
    // The same hooks run on every render; only a missing router (or a test's partial mock) makes them throw.
    // eslint-disable-next-line react-hooks/rules-of-hooks
    params = useParams<{ org?: string; ws?: string }>();
    // eslint-disable-next-line react-hooks/rules-of-hooks
    pathname = usePathname();
  } catch {
    // Not inside a router (or hooks not provided): no place.
  }
  return { org: params?.org, ws: params?.ws, pathname: pathname ?? "" };
}

/** What a page needs to decide about depth: the person's floor from their role, and their switch. */
export interface AltitudeState {
  floor: Altitude;
  more: boolean;
  /** Is a control of this level shown? `allowed` is the role cap the caller already knows (false stays hidden). */
  allows: (level: Exclude<Altitude, "use">, allowed?: boolean) => boolean;
}

/**
 * (P5-REFINE-3, D-406) The viewer's altitude: role sets the floor (instance admin, organization owner or admin, owner of
 * a team workspace → Set up; the rest → Use; instance admin inside Administration → Expert), the per-person switch
 * widens it. Every query here is already cached by the shell. While a role loads the floor is Use, so nothing deeper
 * than the person's role flashes in.
 */
export function useAltitude(at: { wsId?: string; orgId?: string } = {}): AltitudeState {
  // A component rendered without a QueryClient (a unit test of one control) has no role or switch: Use. The presence of
  // the provider never changes during a component's life, so the hook order is stable.
  const hasClient = useContext(QueryClientContext) !== undefined;
  // eslint-disable-next-line react-hooks/rules-of-hooks
  return hasClient ? useLiveAltitude(at) : STATIC_USE;
}

const STATIC_USE: AltitudeState = { floor: "use", more: false, allows: (level, allowed = true) => allowed && altitudeAllows(level, "use", false) };

function useLiveAltitude(at: { wsId?: string; orgId?: string }): AltitudeState {
  const { org, ws, pathname } = useRoutePlace();
  const me = useMe();
  const resolved = useResolve(org, ws);
  // The shell passes the ids it already has; a page takes them from the route.
  const wsQ = useWorkspace(at.wsId ?? resolved.data?.workspace?.id);
  const orgQ = useOrg(at.orgId ?? resolved.data?.org.id ?? wsQ.data?.org_id);
  // `GET /v1/workspaces/{ws}` does not always say whose a personal workspace is; the organization's list does (cached).
  // Until it answers, a workspace owner is treated as the owner of a personal one: nothing deeper flashes in.
  const list = useWorkspaces(wsQ.data ? wsQ.data.org_id : undefined);
  const personal =
    !!wsQ.data?.personal_of || (!!wsQ.data && (list.isPending || !!list.data?.items.find((x) => x.id === wsQ.data?.id)?.personal_of));
  const { more } = useMoreControls();
  const instanceAdmin = me.data?.user.is_instance_admin === true;
  const inAdmin = pathname === "/admin" || pathname.startsWith("/admin/");
  const orgRole = orgQ.data?.my_role;
  const floor = viewerFloor(
    {
      instanceAdmin,
      org: orgRole === "owner" || orgRole === "admin" || orgRole === "member" ? orgRole : undefined,
      workspace: wsQ.data?.my_role,
      personalWorkspace: personal,
    },
    inAdmin,
  );
  return useMemo(
    () => ({
      floor,
      more,
      allows: (level, allowed = true) => allowed && altitudeAllows(level, floor, more),
    }),
    [floor, more],
  );
}

interface Scope {
  /** Add `n` hidden controls under `key`; the returned function takes them away again. */
  add: (key: string, n: number) => () => void;
  count: number;
}

const ScopeContext = createContext<Scope | null>(null);

/** A page's count of hidden controls, for its footer line (`<MoreControlsLine />`). Nesting: the nearest one counts. */
export function AltitudeScope({ children }: { children: ReactNode }) {
  const [hidden, setHidden] = useState<Record<string, number>>({});
  const add = useCallback((key: string, n: number) => {
    setHidden((h) => ({ ...h, [key]: n }));
    return () => setHidden((h) => Object.fromEntries(Object.entries(h).filter(([k]) => k !== key)));
  }, []);
  const count = Object.values(hidden).reduce((a, b) => a + b, 0);
  const value = useMemo(() => ({ add, count }), [add, count]);
  return <ScopeContext.Provider value={value}>{children}</ScopeContext.Provider>;
}

/** Report `n` hidden controls to the page's scope while mounted (a page list that dropped some entries, say). */
export function useHiddenControls(n: number) {
  const scope = useContext(ScopeContext);
  const key = useId();
  const add = scope?.add;
  useEffect(() => {
    if (!add || n <= 0) return;
    return add(key, n);
  }, [add, key, n]);
}

export function useHiddenCount(): number {
  return useContext(ScopeContext)?.count ?? 0;
}

const OPENED_KEY = "aurora.altitude.opened";
const HINT_DISMISSED_KEY = "aurora.altitude.hint-dismissed";
/** After this many in-place openings the one-time hint offers the switch. */
export const HINT_AFTER_OPENED = 3;

/** How many in-place collapsed lines the person has opened (a counter in `localStorage`; 0 when it is unavailable). */
export function openedCount(): number {
  try {
    const n = Number(window.localStorage.getItem(OPENED_KEY));
    return Number.isFinite(n) && n > 0 ? n : 0;
  } catch {
    return 0;
  }
}

export function noteOpened(): number {
  const n = openedCount() + 1;
  try {
    window.localStorage.setItem(OPENED_KEY, String(n));
  } catch {
    /* no storage: the hint never shows */
  }
  return n;
}

export function hintDismissed(): boolean {
  try {
    return window.localStorage.getItem(HINT_DISMISSED_KEY) === "1";
  } catch {
    return true;
  }
}

export function dismissHint() {
  try {
    window.localStorage.setItem(HINT_DISMISSED_KEY, "1");
  } catch {
    /* ignore */
  }
}
