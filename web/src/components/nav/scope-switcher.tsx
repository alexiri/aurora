"use client";

import Link from "next/link";
import { useEffect, useMemo, useRef, useState, type KeyboardEvent, type ReactNode } from "react";

import { tintFor } from "@/components/agent-avatar";
import { CreateWorkspaceDialog } from "@/components/create-workspace-dialog";
import { ArrowLeftIcon, ArrowRightIcon, CheckIcon, ChevronsUpDownIcon, PlusIcon, SearchIcon } from "@/components/ui/icons";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { useMe, useResolve, useWorkspaces } from "@/lib/api/queries";
import { orgRoutes, topicHref, wsRoutes } from "@/lib/routes";
import { cn } from "@/lib/utils";
import { workspaceDisplayName } from "@/lib/workspace-name";

/** How many of an organization's workspaces the menu lists before "All n in <Org>" (D-389). */
const RECENT_LIMIT = 5;
/** How many other organizations the menu lists before "All n organizations". */
const ORG_LIMIT = 3;

interface WsItem {
  slug: string;
  name: string;
  role: string;
  personal?: boolean;
  personal_of?: { id: string; name: string } | null;
}
interface OrgItem {
  id?: string;
  slug: string;
  name: string;
  role?: string;
  workspaces: WsItem[];
}

const roleLabel = (r: string) => (r ? r.charAt(0).toUpperCase() + r.slice(1) : "");

/* Last-used order, per person, in this browser only (the API has no "recently used"). */
const recentKey = (userId: string | undefined) => `aurora.recent-scopes:${userId ?? "anon"}`;
function readRecent(userId: string | undefined): string[] {
  try {
    const raw = window.localStorage.getItem(recentKey(userId));
    const parsed: unknown = raw ? JSON.parse(raw) : [];
    return Array.isArray(parsed) ? parsed.filter((x): x is string => typeof x === "string") : [];
  } catch {
    return [];
  }
}
function writeRecent(userId: string | undefined, key: string) {
  try {
    const next = [key, ...readRecent(userId).filter((k) => k !== key)].slice(0, 50);
    window.localStorage.setItem(recentKey(userId), JSON.stringify(next));
  } catch {
    // Storage is blocked or full: the list keeps its default order.
  }
}

function Chip({ name, tintKey, square }: { name: string; tintKey?: string; square?: boolean }) {
  return (
    <span
      className={cn(
        "inline-flex shrink-0 items-center justify-center font-semibold uppercase",
        square ? "size-7 rounded-md text-label" : "size-7 rounded-full text-xs",
        tintFor(tintKey ?? name),
      )}
    >
      {square ? name.trim().slice(0, 2) || "?" : name.trim().charAt(0) || "?"}
    </span>
  );
}

const rowClass =
  "flex min-h-9 w-full items-center gap-2 rounded-md px-2 py-1.5 text-left text-sm text-ink outline-none hover:bg-subtle focus-visible:bg-subtle";

function Heading({ children }: { children: ReactNode }) {
  return <p className="px-2 pb-1 pt-2 text-label font-semibold uppercase tracking-label text-muted">{children}</p>;
}

type View = { kind: "root" } | { kind: "org"; slug: string } | { kind: "orgs" };

/**
 * D-389: the one control at the top of the rail that says where you are and moves you. The button names the
 * workspace (and its organization); the menu finds a workspace or an organization, lists the current
 * organization's most recently used workspaces and the other organizations, and scales by capping every list
 * and offering the full pages.
 */
export function ScopeSwitcher({ orgSlug, wsSlug }: { orgSlug?: string; wsSlug?: string }) {
  const me = useMe();
  const resolved = useResolve(orgSlug);
  const currentWorkspaces = useWorkspaces(resolved.data?.org.id);
  const viewer = me.data?.user;
  const instanceAdmin = viewer?.is_instance_admin === true;
  const [open, setOpen] = useState(false);
  const [view, setView] = useState<View>({ kind: "root" });
  const [query, setQuery] = useState("");
  const [creating, setCreating] = useState(false);
  const panel = useRef<HTMLDivElement>(null);
  const search = useRef<HTMLInputElement>(null);

  // Everything the person can use, from the session; the current organization's own list (complete for an
  // instance admin who has no membership there) replaces its session entry once loaded.
  const orgs: OrgItem[] = useMemo(() => {
    const list: OrgItem[] = (me.data?.memberships ?? []).map((m) => ({
      id: m.org.id,
      slug: m.org.slug,
      name: m.org.name ?? m.org.slug,
      role: m.role,
      workspaces: m.workspaces.map((w) => ({
        slug: w.slug,
        name: w.name ?? w.slug,
        role: w.my_role ?? "member",
        personal: w.personal,
      })),
    }));
    if (orgSlug && resolved.data) {
      const fresh: WsItem[] | undefined = currentWorkspaces.data?.items.map((w) => ({
        slug: w.slug,
        name: w.name,
        role: w.my_role,
        personal: list.find((o) => o.slug === orgSlug)?.workspaces.some((x) => x.slug === w.slug && x.personal === true),
        personal_of: w.personal_of,
      }));
      const at = list.findIndex((o) => o.slug === orgSlug);
      if (at >= 0) {
        if (fresh) list[at] = { ...list[at]!, workspaces: fresh };
      } else {
        list.unshift({ id: resolved.data.org.id, slug: orgSlug, name: resolved.data.org.name, workspaces: fresh ?? [] });
      }
    }
    return list;
  }, [me.data, orgSlug, resolved.data, currentWorkspaces.data]);

  const label = (w: WsItem) => workspaceDisplayName(w, viewer);
  const current = orgs.find((o) => o.slug === orgSlug);
  const currentWs = current?.workspaces.find((w) => w.slug === wsSlug);
  useEffect(() => {
    if (orgSlug && wsSlug && viewer) writeRecent(viewer.id, `${orgSlug}/${wsSlug}`);
  }, [orgSlug, wsSlug, viewer]);
  // Read once each time the menu opens (the order must not shuffle while it is open).
  const viewerId = viewer?.id;
  const recent = useMemo(() => (open ? readRecent(viewerId) : []), [open, viewerId]);

  const byRecent = (o: OrgItem): WsItem[] => {
    const rank = (w: WsItem) => {
      const i = recent.indexOf(`${o.slug}/${w.slug}`);
      return i < 0 ? Number.MAX_SAFE_INTEGER : i;
    };
    return [...o.workspaces].sort(
      (a, b) =>
        rank(a) - rank(b) ||
        Number(b.personal === true || b.personal_of?.id === viewer?.id) - Number(a.personal === true || a.personal_of?.id === viewer?.id) ||
        label(a).localeCompare(label(b)),
    );
  };
  const orgRank = (o: OrgItem) => {
    const i = recent.findIndex((k) => k.startsWith(`${o.slug}/`));
    return i < 0 ? Number.MAX_SAFE_INTEGER : i;
  };
  const others = orgs.filter((o) => o.slug !== orgSlug).sort((a, b) => orgRank(a) - orgRank(b));
  const canSeeSettings = (o: OrgItem) => instanceAdmin || o.role === "owner" || o.role === "admin";
  const canCreate = current !== undefined && current.id !== undefined && canSeeSettings(current);

  function close() {
    setOpen(false);
  }
  function onOpenChange(next: boolean) {
    setOpen(next);
    if (!next) {
      setView({ kind: "root" });
      setQuery("");
    }
  }

  // Arrow keys move between rows; typing anywhere goes to the search field.
  function onKeyDown(e: KeyboardEvent<HTMLDivElement>) {
    const rows = [...(panel.current?.querySelectorAll<HTMLElement>("[data-scope-row]") ?? [])];
    const at = rows.indexOf(document.activeElement as HTMLElement);
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      const next = e.key === "ArrowDown" ? Math.min(rows.length - 1, at + 1) : Math.max(0, at - 1);
      rows[next]?.focus();
    } else if (e.key.length === 1 && !e.ctrlKey && !e.metaKey && !e.altKey && document.activeElement !== search.current) {
      if (e.key !== " " || (document.activeElement as HTMLElement | null)?.tagName !== "BUTTON") search.current?.focus();
    }
  }

  const q = query.trim().toLowerCase();
  const wsHits = q
    ? orgs.flatMap((o) =>
        o.workspaces
          .filter((w) => label(w).toLowerCase().includes(q) || w.name.toLowerCase().includes(q))
          .map((w) => ({ o, w })),
      )
    : [];
  const orgHits = q ? orgs.filter((o) => o.name.toLowerCase().includes(q) || o.slug.toLowerCase().includes(q)) : [];

  const wsRow = (o: OrgItem, w: WsItem, withOrg = false) => {
    const isCurrent = o.slug === orgSlug && w.slug === wsSlug;
    return (
      <li key={`${o.slug}/${w.slug}`}>
        <Link
          href={wsRoutes(o.slug, w.slug).home()}
          data-scope-row
          aria-current={isCurrent ? "true" : undefined}
          onClick={close}
          className={rowClass}
        >
          <Chip name={label(w)} tintKey={w.slug} />
          <span className="min-w-0 flex-1">
            <span className={cn("block truncate", isCurrent && "font-semibold")}>{label(w)}</span>
            <span className="block truncate text-xs text-muted">{withOrg ? `${o.name} · ${roleLabel(w.role)}` : roleLabel(w.role)}</span>
          </span>
          {isCurrent ? <CheckIcon className="size-4 shrink-0 text-accent" /> : null}
        </Link>
      </li>
    );
  };

  const orgRow = (o: OrgItem) => (
    <li key={o.slug}>
      <button type="button" data-scope-row onClick={() => { setView({ kind: "org", slug: o.slug }); setQuery(""); }} className={rowClass}>
        <Chip name={o.name} tintKey={o.slug} square />
        <span className="min-w-0 flex-1 truncate">
          <span className="font-semibold">{o.name}</span>
          <span className="text-muted"> · {o.workspaces.length} {o.workspaces.length === 1 ? "workspace" : "workspaces"}</span>
        </span>
        <ArrowRightIcon className="size-4 shrink-0 text-muted" />
      </button>
    </li>
  );

  const workspaceList = (o: OrgItem) => {
    const sorted = byRecent(o);
    return (
      <>
        <ul>{sorted.slice(0, RECENT_LIMIT).map((w) => wsRow(o, w))}</ul>
        {sorted.length > RECENT_LIMIT ? (
          <Link href={orgRoutes(o.slug).home()} data-scope-row onClick={close} className={cn(rowClass, "pl-11")}>
            All {sorted.length} in {o.name}
          </Link>
        ) : null}
        {canSeeSettings(o) ? (
          <Link href={topicHref("org", "settings", o.slug)} data-scope-row onClick={close} className={cn(rowClass, "pl-11 text-ink-2")}>
            Organization settings
          </Link>
        ) : null}
      </>
    );
  };

  let body: ReactNode;
  if (q) {
    body = (
      <>
        {wsHits.length > 0 ? (
          <>
            <Heading>Workspaces</Heading>
            <ul>{wsHits.map(({ o, w }) => wsRow(o, w, true))}</ul>
          </>
        ) : null}
        {orgHits.length > 0 ? (
          <>
            <Heading>Organizations</Heading>
            <ul>{orgHits.map(orgRow)}</ul>
          </>
        ) : null}
        {wsHits.length === 0 && orgHits.length === 0 ? <p className="px-2 py-3 text-sm text-muted">Nothing matches “{query.trim()}”.</p> : null}
      </>
    );
  } else if (view.kind === "org") {
    const o = orgs.find((x) => x.slug === view.slug);
    body = o ? (
      <>
        <button type="button" data-scope-row onClick={() => setView({ kind: "root" })} className={cn(rowClass, "font-semibold")}>
          <ArrowLeftIcon className="size-4 shrink-0 text-muted" />
          <span className="truncate">{o.name}</span>
        </button>
        {workspaceList(o)}
      </>
    ) : null;
  } else if (view.kind === "orgs") {
    body = (
      <>
        <button type="button" data-scope-row onClick={() => setView({ kind: "root" })} className={cn(rowClass, "font-semibold")}>
          <ArrowLeftIcon className="size-4 shrink-0 text-muted" />
          <span>All {orgs.length} organizations</span>
        </button>
        <ul>{[...orgs].sort((a, b) => a.name.localeCompare(b.name)).map(orgRow)}</ul>
      </>
    );
  } else {
    body = (
      <>
        {current ? (
          <>
            <Heading>{current.name}</Heading>
            {workspaceList(current)}
          </>
        ) : null}
        {others.length > 0 ? (
          <>
            <div className="my-1 h-px bg-line" />
            <Heading>{current ? "Other organizations" : "Organizations"}</Heading>
            <ul>{others.slice(0, ORG_LIMIT).map(orgRow)}</ul>
          </>
        ) : null}
        {orgs.length > ORG_LIMIT ? (
          <button type="button" data-scope-row onClick={() => setView({ kind: "orgs" })} className={cn(rowClass, "pl-11")}>
            All {orgs.length} organizations
          </button>
        ) : null}
      </>
    );
  }

  // ---- the button ----
  const onWorkspace = orgSlug !== undefined && wsSlug !== undefined;
  const first = onWorkspace
    ? currentWs
      ? label(currentWs)
      : (resolved.data?.workspace?.name ?? wsSlug)
    : orgSlug
      ? (current?.name ?? resolved.data?.org.name ?? orgSlug)
      : "Choose a workspace";
  const second = onWorkspace ? (current?.name ?? resolved.data?.org.name ?? orgSlug) : orgSlug ? "Organization" : undefined;
  const face = (
    <>
      {first && orgSlug ? <Chip name={first} tintKey={onWorkspace ? wsSlug : orgSlug} square={!onWorkspace} /> : null}
      {first && orgSlug ? " " : null}
      <span className="min-w-0 flex-1 text-left">
        <span className="block truncate text-sm font-semibold text-ink">{first}</span>
        {second ? <span className="block truncate text-xs text-muted">{second}</span> : null}
      </span>
    </>
  );
  const frame = "flex min-h-11 w-full items-center gap-2.5 rounded-control border border-line bg-surface px-2.5 py-1.5";

  const single = orgs.length === 1 && orgs[0]!.workspaces.length === 1 && !canCreate && me.data !== undefined && !instanceAdmin;
  if (single) {
    return (
      <div data-testid="scope-switcher" className={frame}>
        {face}
      </div>
    );
  }

  return (
    <div data-testid="scope-switcher">
      <Popover open={open} onOpenChange={onOpenChange}>
        <PopoverTrigger asChild>
          <button type="button" className={cn(frame, "hover:bg-subtle")}>
            {face}
            <ChevronsUpDownIcon className="size-4 shrink-0 text-muted" />
          </button>
        </PopoverTrigger>
        <PopoverContent
          align="start"
          sideOffset={6}
          aria-label="Switch workspace or organization"
          className="w-72 p-1.5"
          onOpenAutoFocus={(e) => {
            e.preventDefault();
            search.current?.focus();
          }}
        >
          <div ref={panel} onKeyDown={onKeyDown}>
            <label className="relative block">
              <span className="sr-only">Find a workspace or organization</span>
              <SearchIcon className="pointer-events-none absolute left-2.5 top-1/2 size-4 -translate-y-1/2 text-muted" />
              <input
                ref={search}
                data-scope-row
                type="search"
                value={query}
                onChange={(e) => setQuery(e.target.value)}
                placeholder="Find a workspace or organization"
                autoComplete="off"
                className="h-9 w-full rounded-control border border-line bg-surface pl-8 pr-2 text-sm text-ink placeholder:text-muted"
              />
            </label>
            {body}
            {canCreate && !q ? (
              <>
                <div className="my-1 h-px bg-line" />
                <button
                  type="button"
                  data-scope-row
                  onClick={() => {
                    close();
                    setCreating(true);
                  }}
                  className={rowClass}
                >
                  <PlusIcon className="size-4 shrink-0 text-muted" />
                  New workspace in {current!.name}
                </button>
              </>
            ) : null}
          </div>
        </PopoverContent>
      </Popover>
      {canCreate ? <CreateWorkspaceDialog orgId={current!.id!} orgSlug={current!.slug} open={creating} onOpenChange={setCreating} /> : null}
    </div>
  );
}
