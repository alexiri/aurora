"use client";

import Link from "next/link";
import { useEffect, useId, useRef, useState, type KeyboardEvent, type ReactNode, type RefObject } from "react";

import { tintFor } from "@/components/agent-avatar";
import { CountBadge } from "@/components/attention/count-badge";
import { Badge } from "@/components/ui/badge";
import { inputClass } from "@/components/ui/field";
import { ArrowRightIcon, CheckIcon, PlusIcon, SearchIcon, ShieldIcon } from "@/components/ui/icons";
import { useAdminBadge, useOrgBadge } from "@/lib/api/attention";
import { capitalised, enterable, ORG_LIMIT, orgTarget, otherOrgs, RECENT_LIMIT, type PlaceOrg, type PlaceWorkspace } from "@/lib/nav/places";
import type { ScopeMembership } from "@/lib/nav/recent-scopes";
import { adminRoutes, orgRoutes, wsRoutes } from "@/lib/routes";
import { cn } from "@/lib/utils";

/** The places the picker is opened from. */
export type PlaceKind = "workspace" | "org" | "admin" | "account";
export interface Place {
  kind: PlaceKind;
  orgSlug?: string;
  wsSlug?: string;
}

export interface PickerModel {
  orgs: PlaceOrg[];
  /** The organization the picker is about: the place's, or (Administration, Your account) the one you came from. */
  current: PlaceOrg | undefined;
  instanceAdmin: boolean;
  userId: string | undefined;
  memberships: ScopeMembership[] | undefined;
  recent: string[];
  canCreateWorkspace: boolean;
  canCreateOrg: boolean;
  /** There are more than six places: the picker leads with a search field. */
  search: boolean;
}

/** A place's round mark: an organization's two letters (`org`), else one initial. */
export function PlaceMark({ name, tintKey, org }: { name: string; tintKey?: string; org?: boolean }) {
  return (
    <span className={cn("inline-flex size-7 shrink-0 items-center justify-center rounded-full font-semibold uppercase", org ? "text-label" : "text-xs", tintFor(tintKey ?? name))}>
      {org ? name.trim().slice(0, 2) || "?" : name.trim().charAt(0) || "?"}
    </span>
  );
}

export function AdminMark() {
  return (
    <span className="inline-flex size-7 shrink-0 items-center justify-center rounded-full bg-subtle text-ink">
      <ShieldIcon className="size-4" />
    </span>
  );
}

const rowClass =
  "flex min-h-11 w-full items-center gap-2.5 rounded-control px-2.5 py-1.5 text-left text-sm text-ink outline-none hover:bg-subtle focus-visible:bg-subtle";

function Heading({ id, children }: { id: string; children: ReactNode }) {
  return (
    <p id={id} className="px-2.5 pb-1 pt-2 text-label font-semibold uppercase tracking-label text-muted">
      {children}
    </p>
  );
}

function RowText({ title, caption, strong = true }: { title: ReactNode; caption?: ReactNode; strong?: boolean }) {
  return (
    <span className="min-w-0 flex-1">
      <span className={cn("block truncate", strong && "font-semibold")}>{title}</span>{" "}
      {caption ? <span className="block text-xs text-muted">{caption}</span> : null}
    </span>
  );
}

const Current = () => <CheckIcon className="size-4 shrink-0 text-accent" />;
/** A row with no mark lines its words up with the others. */
const Indent = () => <span aria-hidden="true" className="w-7 shrink-0" />;

/** "Manage <org>"'s row: what it holds, the person's role, and what waits there for them (C-1082). */
function ManageRow({ org, current, onClose }: { org: PlaceOrg; current: boolean; onClose: () => void }) {
  const badge = useOrgBadge(org.id);
  const role = org.role === "owner" || org.role === "admin" ? capitalised(org.role) : undefined;
  return (
    <Link href={orgRoutes(org.slug).home()} data-scope-row aria-current={current ? "true" : undefined} onClick={onClose} className={rowClass}>
      <PlaceMark name={org.name} tintKey={org.slug} org />{" "}
      <RowText title={`Manage ${org.name}`} caption="People, apps, AI models, data" />{" "}
      {role ? <Badge tone="accent">{role}</Badge> : null}{" "}
      <CountBadge n={badge?.count} actionId="manage-switcher-entry" testId="manage-switcher-badge" />
      {current ? <Current /> : null}
    </Link>
  );
}

function AdministrationRow({ current, onClose }: { current: boolean; onClose: () => void }) {
  const badge = useAdminBadge();
  return (
    <Link href={adminRoutes.home()} data-scope-row aria-current={current ? "true" : undefined} onClick={onClose} className={rowClass}>
      <AdminMark /> <RowText title="Administration" caption="Settings for everyone on this Aurora" />{" "}
      <CountBadge n={badge?.count} actionId="admin-nav-entry" testId="picker-admin-badge" />
      {current ? <Current /> : null}
    </Link>
  );
}

interface Hit {
  key: string;
  href: string;
  title: string;
  caption: string;
  mark: ReactNode;
}

const orgRoleWords = (o: PlaceOrg, instanceAdmin: boolean) => (o.role ? capitalised(o.role) : instanceAdmin ? "Instance admin" : "Member");

/** What a search finds, across organizations, grouped by organization: workspaces, Manage entries and Administration. */
function search(model: PickerModel, q: string): { heading: string; hits: Hit[] }[] {
  const has = (s: string) => s.toLowerCase().includes(q);
  const first = model.current?.slug;
  const groups = [...model.orgs]
    .filter(enterable)
    .sort((a, b) => Number(b.slug === first) - Number(a.slug === first) || a.name.localeCompare(b.name))
    .map((o) => {
      const hits: Hit[] = o.workspaces
        .filter((w) => has(w.label) || has(w.stored) || has(o.name))
        .map((w) => ({
          key: `ws:${o.slug}/${w.slug}`,
          href: wsRoutes(o.slug, w.slug).home(),
          title: w.label,
          caption: `Workspace · ${w.role}`,
          mark: <PlaceMark name={w.mark} tintKey={w.slug} />,
        }));
      const named = has(o.name) || has(o.slug);
      if (o.manage && (named || has(`manage ${o.name}`))) {
        hits.push({
          key: `manage:${o.slug}`,
          href: orgRoutes(o.slug).home(),
          title: `Manage ${o.name}`,
          caption: `Organization · ${orgRoleWords(o, model.instanceAdmin)}`,
          mark: <PlaceMark name={o.name} tintKey={o.slug} org />,
        });
      } else if (!o.manage && named) {
        const href = orgTarget(o, model.userId, model.memberships);
        if (href) {
          hits.push({ key: `org:${o.slug}`, href, title: o.name, caption: `Organization · ${orgRoleWords(o, false)}`, mark: <PlaceMark name={o.name} tintKey={o.slug} org /> });
        }
      }
      return { heading: o.name, hits };
    });
  if (model.instanceAdmin && has("administration")) {
    groups.push({
      heading: "Administration",
      hits: [{ key: "admin", href: adminRoutes.home(), title: "Administration", caption: "Settings for everyone on this Aurora", mark: <AdminMark /> }],
    });
  }
  return groups.filter((g) => g.hits.length > 0);
}

const plural = (n: number, word: string) => `${n} ${word}${n === 1 ? "" : "s"}`;

/**
 * (D-508(f), nav-3a) The place picker's content, the same on a desktop (a popover under the place card) and on a phone
 * (a sheet): search when there are many places, then Workspaces in <org>, Organization, Administration, the other
 * organizations and New — each only when there is something in it. Arrows move, Enter opens, typing searches.
 */
export function PlacePickerBody({
  model,
  place,
  onClose,
  onNewWorkspace,
  onNewOrg,
  searchRef,
}: {
  model: PickerModel;
  place: Place;
  onClose: () => void;
  onNewWorkspace: () => void;
  onNewOrg: () => void;
  searchRef: RefObject<HTMLInputElement | null>;
}) {
  const [query, setQuery] = useState("");
  const [active, setActive] = useState(0);
  const [allWorkspaces, setAllWorkspaces] = useState(false);
  const [allOrgs, setAllOrgs] = useState(false);
  const panel = useRef<HTMLDivElement>(null);
  const id = useId();
  const listId = `${id}-results`;
  const q = model.search ? query.trim().toLowerCase() : "";
  const results = q ? search(model, q) : [];
  const flat = results.flatMap((g) => g.hits);
  const picked = Math.min(active, Math.max(0, flat.length - 1));

  useEffect(() => {
    panel.current?.querySelector<HTMLElement>("[aria-selected='true']")?.scrollIntoView?.({ block: "nearest" });
  }, [picked, q]);

  // Arrow keys move between rows (with a search, they move the highlight and the field keeps the focus); typing anywhere
  // goes to the search field.
  function onKeyDown(e: KeyboardEvent<HTMLDivElement>) {
    const arrow = e.key === "ArrowDown" || e.key === "ArrowUp";
    if (q) {
      if (arrow) {
        e.preventDefault();
        setActive(e.key === "ArrowDown" ? Math.min(flat.length - 1, picked + 1) : Math.max(0, picked - 1));
      } else if (e.key === "Enter" && !e.nativeEvent.isComposing) {
        e.preventDefault();
        panel.current?.querySelector<HTMLElement>("[aria-selected='true']")?.click();
      }
      if (arrow || e.key === "Enter") return;
    }
    const rows = [...(panel.current?.querySelectorAll<HTMLElement>("[data-scope-row]") ?? [])];
    const at = rows.indexOf(document.activeElement as HTMLElement);
    if (arrow) {
      e.preventDefault();
      const next = e.key === "ArrowDown" ? Math.min(rows.length - 1, at + 1) : Math.max(0, at - 1);
      rows[next]?.focus();
    } else if (e.key.length === 1 && e.key !== " " && !e.ctrlKey && !e.metaKey && !e.altKey && document.activeElement !== searchRef.current) {
      searchRef.current?.focus();
    }
  }

  const current = model.current;
  const here = (orgSlug: string, w: PlaceWorkspace) => place.kind === "workspace" && place.orgSlug === orgSlug && place.wsSlug === w.slug;
  const workspaceRows = current ? (allWorkspaces ? current.workspaces : current.workspaces.slice(0, RECENT_LIMIT)) : [];
  // Owners and admins may list every workspace (Manage › Workspaces); a member's "all" is their own, shown here.
  const total = current ? (current.manage ? (current.total ?? current.workspaces.length) : current.workspaces.length) : 0;
  const others = otherOrgs(model.orgs, current?.slug, model.recent);
  const shownOthers = allOrgs ? others : others.slice(0, ORG_LIMIT);

  const groups: ReactNode[] = [];
  if (current && (workspaceRows.length > 0 || (current.manage && total > 0))) {
    groups.push(
      <div key="workspaces" role="group" aria-labelledby={`${id}-ws`}>
        <Heading id={`${id}-ws`}>Workspaces in {current.name}</Heading>
        <ul>
          {workspaceRows.map((w) => (
            <li key={w.slug}>
              <Link href={wsRoutes(current.slug, w.slug).home()} data-scope-row aria-current={here(current.slug, w) ? "true" : undefined} onClick={onClose} className={rowClass}>
                <PlaceMark name={w.mark} tintKey={w.slug} /> <RowText title={w.label} caption={`${w.role} · ${current.name}`} />
                {here(current.slug, w) ? <Current /> : null}
              </Link>
            </li>
          ))}
        </ul>
        {total > workspaceRows.length ? (
          current.manage ? (
            <Link href={orgRoutes(current.slug).workspaces()} data-scope-row onClick={onClose} className={cn(rowClass, "text-ink-2")}>
              <Indent />
              <span className="min-w-0 flex-1">All {total} workspaces…</span>
              <ArrowRightIcon className="size-4 shrink-0" />
            </Link>
          ) : (
            <button type="button" data-scope-row onClick={() => setAllWorkspaces(true)} className={cn(rowClass, "text-ink-2")}>
              <Indent />
              <span className="min-w-0 flex-1">All {total} workspaces…</span>
              <ArrowRightIcon className="size-4 shrink-0" />
            </button>
          )
        ) : null}
      </div>,
    );
  }
  if (current?.manage) {
    groups.push(
      <div key="organization" role="group" aria-labelledby={`${id}-org`}>
        <Heading id={`${id}-org`}>Organization</Heading>
        <ManageRow org={current} current={place.kind === "org" && place.orgSlug === current.slug} onClose={onClose} />
      </div>,
    );
  }
  if (model.instanceAdmin) {
    groups.push(
      <div key="administration" role="group" aria-labelledby={`${id}-admin`}>
        <Heading id={`${id}-admin`}>Administration</Heading>
        <AdministrationRow current={place.kind === "admin"} onClose={onClose} />
      </div>,
    );
  }
  if (others.length > 0) {
    groups.push(
      <div key="others" role="group" aria-labelledby={`${id}-others`}>
        <p id={`${id}-others`} className="flex min-h-9 items-center gap-2.5 px-2.5 text-sm text-ink-2">
          <Indent />
          Other organizations
        </p>
        <ul>
          {shownOthers.map((o) => {
            const opens = o.workspaces.length > 0 ? "opens your last workspace there" : `opens Manage ${o.name}`;
            return (
              <li key={o.slug}>
                <Link href={orgTarget(o, model.userId, model.memberships) ?? "/"} data-scope-row onClick={onClose} className={rowClass}>
                  <PlaceMark name={o.name} tintKey={o.slug} />{" "}
                  <RowText title={o.name} caption={`${orgRoleWords(o, model.instanceAdmin)} · ${opens}`} />
                  <ArrowRightIcon className="size-4 shrink-0 text-ink-2" />
                </Link>
              </li>
            );
          })}
        </ul>
        {others.length > shownOthers.length ? (
          <button type="button" data-scope-row onClick={() => setAllOrgs(true)} className={cn(rowClass, "text-ink-2")}>
            <Indent />
            All {others.length + (current ? 1 : 0)} organizations
          </button>
        ) : null}
      </div>,
    );
  }
  if (model.canCreateWorkspace || model.canCreateOrg) {
    const action = (label: string, run: () => void) => (
      <button
        type="button"
        data-scope-row
        onClick={() => {
          onClose();
          run();
        }}
        className={rowClass}
      >
        <span className="inline-flex w-7 shrink-0 justify-center">
          <PlusIcon className="size-4 text-ink-2" />
        </span>
        {label}
      </button>
    );
    groups.push(
      <div key="new">
        {model.canCreateWorkspace && current ? action(`New workspace in ${current.name}`, onNewWorkspace) : null}
        {model.canCreateOrg ? action("New organization", onNewOrg) : null}
      </div>,
    );
  }

  const searchable = model.orgs.filter(enterable);
  return (
    <div ref={panel} onKeyDown={onKeyDown} className="px-1.5 pb-1.5">
      {model.search ? (
        <label className="sticky top-0 z-10 block bg-surface pb-1 pt-1.5">
          <span className="sr-only">Find a workspace or organization</span>
          <SearchIcon className="pointer-events-none absolute left-3 top-1/2 size-4 -translate-y-1/2 text-muted" />
          <input
            ref={searchRef}
            data-scope-row
            // Not `type="search"`: the browser's own clear mark would sit on the count.
            type="text"
            role="combobox"
            aria-expanded={q ? "true" : "false"}
            aria-controls={q ? listId : undefined}
            aria-activedescendant={q && flat[picked] ? `${listId}-${picked}` : undefined}
            value={query}
            onChange={(e) => {
              setQuery(e.target.value);
              setActive(0);
            }}
            placeholder="Find a workspace or organization"
            autoComplete="off"
            className={cn(inputClass, "min-h-10 py-1 pl-9", q ? "pr-3 sm:pr-48" : "pr-2")}
          />
          {q ? (
            <span className="pointer-events-none absolute right-3 top-1/2 hidden -translate-y-1/2 text-xs text-muted sm:inline">
              {plural(searchable.reduce((n, o) => n + o.workspaces.length, 0), "workspace")}, {plural(searchable.length, "organization")}
            </span>
          ) : null}
        </label>
      ) : (
        <div className="pt-1.5" />
      )}
      {q ? (
        <>
          <div id={listId} role="listbox" aria-label="Places found">
            {results.map((g, n) => (
              // The id is numbered: an organization's name has spaces, which `aria-labelledby` reads as several ids.
              <div key={g.heading} role="group" aria-labelledby={`${listId}-g${n}`}>
                <Heading id={`${listId}-g${n}`}>{g.heading}</Heading>
                {g.hits.map((h) => {
                  const at = flat.indexOf(h);
                  const on = at === picked;
                  return (
                    <Link
                      key={h.key}
                      id={`${listId}-${at}`}
                      href={h.href}
                      role="option"
                      aria-selected={on}
                      onMouseMove={() => setActive(at)}
                      onClick={onClose}
                      className={cn(rowClass, on && "bg-accent-soft hover:bg-accent-soft")}
                    >
                      {h.mark} <RowText title={h.title} caption={h.caption} />
                      {on ? (
                        <span aria-hidden="true" className="shrink-0 text-xs text-muted">
                          ↵
                        </span>
                      ) : null}
                    </Link>
                  );
                })}
              </div>
            ))}
          </div>
          {flat.length === 0 ? (
            <p role="status" className="px-2.5 py-3 text-sm text-muted">
              Nothing matches “{query.trim()}”.
            </p>
          ) : null}
          <p className="mt-1 border-t border-line px-2.5 pb-0.5 pt-2 text-xs text-muted">↑↓ to move · ⏎ to open · Esc to close</p>
        </>
      ) : (
        groups.map((g, i) => (
          <div key={(g as { key: string }).key}>
            {i > 0 ? <div className="my-1 h-px bg-line" /> : null}
            {g}
          </div>
        ))
      )}
    </div>
  );
}

/** Is there anywhere else to go, or anything to make? Without it the place card has no menu. */
export function hasSomewhereElse(model: PickerModel, place: Place): boolean {
  if (model.canCreateWorkspace || model.canCreateOrg) return true;
  if (model.instanceAdmin && place.kind !== "admin") return true;
  if (otherOrgs(model.orgs, model.current?.slug, model.recent).length > 0) return true;
  const c = model.current;
  if (!c) return false;
  if (c.manage && !(place.kind === "org" && place.orgSlug === c.slug)) return true;
  return c.workspaces.some((w) => !(place.kind === "workspace" && place.orgSlug === c.slug && place.wsSlug === w.slug));
}
