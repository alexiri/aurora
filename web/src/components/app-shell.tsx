"use client";

import * as DialogPrimitive from "@radix-ui/react-dialog";
import Link from "next/link";
import { useParams, usePathname, useRouter } from "next/navigation";
import { useCallback, useState, useSyncExternalStore, type ComponentType, type ReactNode, type SVGProps } from "react";

import { clearDrafts } from "@/components/chat/composer/drafts";
import { logout, useInstance, useMe, useOrg, useResolve, useWorkspace } from "@/lib/api/queries";
import { useAltitude } from "@/lib/altitude";
import { useMoreControls } from "@/lib/api/use-more-controls";
import { visibleTopics } from "@/lib/api/roles";
import { loginUrl, useUnauthorizedRedirect } from "@/lib/auth/use-unauthorized-redirect";
import { ApiError } from "@/lib/api/client";
import { personName, workspaceDisplayName } from "@/lib/workspace-name";
import { backLink, lastWorkspace, usePlaceRecord, useRecordPlace } from "@/lib/nav/manage-return";
import { removeThisDevicePush, usePushResync } from "@/lib/web-push";
import { accountRoutes, ADMIN_RAIL_GROUPS, ADMIN_RAIL_LABEL, adminRoutes, ORG_RAIL_GROUPS, orgRoutes, TOPIC_LABEL, topicHref, wsRoutes } from "@/lib/routes";
import { useTimezoneAutofill } from "@/lib/timezone";
import { cn } from "@/lib/utils";

import { useOrgBadge, useRefreshAttentionOnMutation, useWorkspaceCounts } from "@/lib/api/attention";
import { AttentionBadge } from "./admin/attention-badge";
import { CountBadge } from "./attention/count-badge";
import { PlaceBack } from "./nav/place-card";
import type { Place } from "./nav/place-picker";
import { placeOf } from "@/lib/nav/places";
import { ApprovalNotifier } from "./approvals/approval-notifier";
import { FrozenEntityBanner } from "./freeze/frozen-entity-banner";
import { EmergencyBanners } from "./emergency/emergency-banners";
import { EMERGENCY_TOPICS, isEmergencyReader } from "@/lib/emergency";
import { CommandPalette, openPalette, openShortcutSheet } from "./palette/command-palette";
import { ScopeSwitcher, TopBarPlace, usePickerModel } from "./nav/scope-switcher";
import { ShortcutsHost } from "./shortcuts/shortcuts-host";
import { isMac, keysLabel, SHORTCUTS } from "@/lib/shortcuts";
import { Altitude } from "./ui/altitude";
import { Tour } from "./onboarding/tour";
import { startTour } from "./onboarding/tour-events";
import { ThemeSwitch } from "./theme/theme-switch";
import { Avatar, tintFor } from "./agent-avatar";
import { NotFoundState, type NotFoundKind } from "./not-found-state";
import { AuroraLogo } from "./ui/aurora-logo";
import { UndoToastHost } from "./ui/undo-toast";
import {
  ActivityIcon,
  BellIcon,
  BotIcon,
  BookIcon,
  BuildingIcon,
  CheckCircleIcon,
  ChevronsUpDownIcon,
  DatabaseIcon,
  GlobeIcon,
  GridIcon,
  HomeIcon,
  KeyIcon,
  LinkIcon,
  LockIcon,
  LogOutIcon,
  MenuIcon,
  KeyboardIcon,
  MessageIcon,
  PlugIcon,
  SearchIcon,
  ScaleIcon,
  ScrollIcon,
  ServerIcon,
  SettingsIcon,
  ShieldIcon,
  SparkIcon,
  TrashIcon,
  UserIcon,
  UsersIcon,
  XIcon,
} from "./ui/icons";
import { Menu, MenuContent, MenuItem, MenuSeparator, MenuSwitchItem, MenuTrigger } from "./ui/menu";

type IconType = ComponentType<SVGProps<SVGSVGElement>>;

function NavItem({
  href,
  label,
  active,
  icon: Icon,
  badge,
  tour,
}: {
  href: string;
  label: string;
  active: boolean;
  icon: IconType;
  badge?: ReactNode;
  /** `data-tour` of the link (a topic id); the tour points at it. */
  tour?: string;
}) {
  return (
    <li>
      <Link
        href={href}
        aria-current={active ? "page" : undefined}
        data-tour={tour ?? label.toLowerCase().replace(/\s+/g, "-")}
        className={cn(
          "flex min-h-9 items-center gap-2.5 rounded-control px-2.5 py-1.5 text-sm",
          active ? "bg-subtle font-semibold text-ink" : "text-ink-2 hover:bg-subtle hover:text-ink",
        )}
      >
        <Icon className={cn("size-4 shrink-0", active ? "text-accent" : "text-muted")} />
        <span className="min-w-0 flex-1 truncate">{label}</span>
        {badge}
      </Link>
    </li>
  );
}

const noopSubscribe = () => () => {};

/** (P5-REFINE-19) "Search · ⌘K" in the rail's footer: opens the command palette; the shortcut is muted text beside it. */
function SearchButton() {
  // The server cannot know the platform: the first render says Ctrl, the browser's own then replaces it.
  const mac = useSyncExternalStore(noopSubscribe, () => isMac(), () => false);
  return (
    <button
      type="button"
      onClick={() => openPalette()}
      data-tour="search"
      className="flex min-h-9 w-full items-center gap-2.5 rounded-control px-2.5 py-1.5 text-left text-sm text-ink-2 hover:bg-subtle hover:text-ink"
    >
      <SearchIcon className="size-4 shrink-0 text-muted" />
      <span className="min-w-0 flex-1 truncate">Search</span>
      <span aria-hidden="true" className="shrink-0 text-xs text-muted">
        {keysLabel(SHORTCUTS.find((s) => s.id === "palette")!.keys, mac)}
      </span>
    </button>
  );
}

function UserMenu() {
  const me = useMe();
  const router = useRouter();
  const name = me.data ? personName(me.data.user.display_name) : undefined;
  const more = useMoreControls();
  // The tour points at the workspace navigation: from a page without it (Account, Admin) go to the landing page first.
  function onTour() {
    const here = [...document.querySelectorAll("[data-tour='agents']")].some((el) => el.getClientRects().length > 0 || window.matchMedia("(max-width: 767px)").matches);
    if (here) startTour();
    else router.push("/?tour=1");
  }
  async function onLogout() {
    // Composer drafts belong to the signed-in person (P5-CHAT-3): they go with the session.
    clearDrafts();
    try {
      // (C-836) This device's push registration goes before the session does (best effort, bounded).
      await removeThisDevicePush();
      await logout();
    } finally {
      window.location.assign(loginUrl("/"));
    }
  }
  const itemClass = "gap-2.5";
  return (
    <Menu>
      <MenuTrigger asChild>
        <button
          type="button"
          className="flex min-h-11 w-full items-center gap-2.5 rounded-control px-2.5 py-1.5 text-left hover:bg-subtle"
          // No aria-label: the name comes from the visible text (WCAG 2.5.3).
        >
          <Avatar name={name ?? "?"} tintKey={me.data?.user.email} size={28} />
          <span className="min-w-0 flex-1">
            <span className="block truncate text-sm font-semibold text-ink">{name ?? "Account"}</span>
            {me.data?.user.is_instance_admin ? <span className="block text-xs text-muted">Instance admin</span> : null}
          </span>
          <ChevronsUpDownIcon className="size-4 shrink-0 text-muted" />
        </button>
      </MenuTrigger>
      <MenuContent align="start" side="top" className="w-[var(--radix-dropdown-menu-trigger-width)] min-w-60">
        {me.data ? (
          <div className="flex items-center gap-2.5 px-2 py-2">
            <Avatar name={personName(me.data.user.display_name)} tintKey={me.data.user.email} size={32} />
            <div className="min-w-0">
              <p className="truncate text-sm font-semibold text-ink">{personName(me.data.user.display_name)}</p>
              <p className="truncate text-xs text-muted">{me.data.user.email}</p>
            </div>
          </div>
        ) : null}
        <MenuSeparator />
        <MenuItem asChild className={itemClass}>
          <Link href={accountRoutes.profile}>
            <UserIcon className="size-4 shrink-0 text-muted" />
            Your account
          </Link>
        </MenuItem>
        {/* C-962: Administration is a labelled entry, here and at the foot of the sidebar, for instance admins. */}
        {me.data?.user.is_instance_admin === true ? (
          <MenuItem asChild className={itemClass}>
            <Link href={adminRoutes.home()}>
              <ShieldIcon className="size-4 shrink-0 text-muted" />
              <span className="min-w-0 flex-1">Administration</span>
              <AttentionBadge />
            </Link>
          </MenuItem>
        ) : null}
        {/* (P5-REFINE-3, D-406) The per-person switch; it does not close the menu. */}
        <MenuSwitchItem checked={more.more} onCheckedChange={more.setMore} disabled={more.pending} className={itemClass}>
          Show more controls
        </MenuSwitchItem>
        {/* (P5-REFINE-19) The shortcut sheet is for whoever turned the switch on. */}
        <Altitude id="kbd" level="expert">
          <MenuItem onSelect={() => openShortcutSheet()} className={itemClass}>
            <KeyboardIcon className="size-4 shrink-0 text-muted" />
            Keyboard shortcuts
          </MenuItem>
        </Altitude>
        <MenuItem onSelect={onTour} className={itemClass}>
          <HomeIcon className="size-4 shrink-0 text-muted" />
          Show me around
        </MenuItem>
        <MenuSeparator />
        <MenuItem onSelect={() => void onLogout()} className={itemClass}>
          <LogOutIcon className="size-4 shrink-0 text-muted" />
          Sign out
        </MenuItem>
      </MenuContent>
    </Menu>
  );
}

const TOPIC_ICON: Record<string, IconType> = {
  overview: HomeIcon,
  workspaces: BuildingIcon,
  agents: BotIcon,
  skills: BookIcon,
  apps: PlugIcon,
  "ai-models": ScaleIcon,
  "internet-access": ShieldIcon,
  approvals: CheckCircleIcon,
  activity: ActivityIcon,
  people: UsersIcon,
  "chat-platforms": MessageIcon,
  data: ScrollIcon,
  platform: ServerIcon,
  "set-in-helm": LockIcon,
  settings: SettingsIcon,
};

/** Overview is the level's own address; every other topic owns the paths below its folder. */
function topicActive(pathname: string, href: string, topic: string): boolean {
  const path = href.split("?")[0]!;
  return topic === "overview" ? pathname === path : pathname === path || pathname.startsWith(`${path}/`);
}

/** Where the Manage menu's icons differ from the shared ones (`docs/design/navigation`, nav-3b). */
const ORG_TOPIC_ICON: Record<string, IconType> = {
  ...TOPIC_ICON,
  workspaces: GridIcon,
  people: UserIcon,
  "ai-models": SparkIcon,
  "internet-access": GlobeIcon,
  data: DatabaseIcon,
};

/** A rail's topics in labelled groups (Administration and Manage <org>); a group with no `label` starts without a heading. */
function GroupedNav<T extends string>({
  label,
  idBase,
  groups,
  visible,
  item,
}: {
  label: string;
  idBase: string;
  groups: readonly { label: string | null; topics: readonly T[] }[];
  visible: ReadonlySet<T>;
  item: (topic: T) => ReactNode;
}) {
  return (
    <nav aria-label={label} className="space-y-3">
      {groups.map((g, i) => {
        const topics = g.topics.filter((t) => visible.has(t));
        if (topics.length === 0) return null;
        const headingId = g.label ? `${idBase}-${g.label.toLowerCase().replace(/\s+/g, "-")}` : undefined;
        return (
          <div key={g.label ?? i} role={g.label ? "group" : undefined} aria-labelledby={headingId} className="space-y-1">
            {g.label ? (
              <p id={headingId} className="px-2.5 text-label font-semibold uppercase tracking-label text-muted">
                {g.label}
              </p>
            ) : null}
            <ul className="space-y-0.5">{topics.map(item)}</ul>
          </div>
        );
      })}
    </nav>
  );
}

/**
 * (D-508(b)) Manage <org>'s menu: it replaces the rail. Owners, admins and instance admins only; nothing is listed until the
 * role is known (a member is on the way to their workspace). The counts are C-1082's: the total on Overview, a part per topic.
 */
export function OrgNavList({ orgSlug, pathname }: { orgSlug: string; pathname: string }) {
  const instanceAdmin = useMe().data?.user.is_instance_admin === true;
  const resolved = useResolve(orgSlug);
  const orgId = resolved.data?.org.id;
  const org = useOrg(orgId);
  const altitude = useAltitude({ orgId });
  const badge = useOrgBadge(orgId);
  const orgName = org.data?.name ?? resolved.data?.org.name;
  if (!orgName) return null;
  const visible = new Set(visibleTopics("org", { instanceAdmin, org: org.data?.my_role }, altitude));
  return (
    <GroupedNav
      label={`Manage ${orgName}`}
      idBase="org-rail"
      groups={ORG_RAIL_GROUPS}
      visible={visible}
      item={(t) => {
        const href = topicHref("org", t, orgSlug);
        return (
          <NavItem
            key={t}
            // Not the bare topic: the tour points at the workspace's navigation.
            tour={`org-${t}`}
            href={href}
            label={TOPIC_LABEL[t]}
            icon={ORG_TOPIC_ICON[t]!}
            active={topicActive(pathname, href, t)}
            badge={<CountBadge n={t === "overview" ? badge?.count : badge?.topics[t]} actionId="manage-nav-entry" testId={`manage-attention-badge-${t}`} />}
          />
        );
      }}
    />
  );
}

/**
 * (P5-REFINE-16 item 19, badges G1, G2, G6) The workspace's numbers, from the one shared hook on C-1082
 * (`useWorkspaceCounts`): `overview` is the whole level, `approvals` the part for that page. Zero for someone who cannot act
 * (the server answers `null` for that scope), so a member sees no badge at all.
 */
export function useWaitingCount(wsId: string | undefined): number {
  return useWorkspaceCounts(wsId).count;
}

/** "1 needs your attention" / "2 need your attention": the words behind a badge. */
export const attentionText = (n: number): string => (n === 1 ? "1 needs your attention" : `${n} need your attention`);

/** The count pill beside a rail entry: no badge at zero, the words for a screen reader inside the link, amber (the viewer can act, D-384). */
function CountPill({ n, testId }: { n: number; testId: string }) {
  if (n === 0) return null;
  return (
    <span
      data-testid={testId}
      className="rounded-full bg-pending-soft px-1.5 text-label font-semibold leading-5 tabular-nums text-pending-ink"
    >
      <span aria-hidden="true">{n}</span>
      <span className="sr-only">{attentionText(n)}</span>
    </span>
  );
}

export function WorkspaceNav({
  base,
  wsId,
  pathname,
}: {
  base: string;
  wsId: string | undefined;
  pathname: string;
}) {
  const instanceAdmin = useMe().data?.user.is_instance_admin === true;
  const w = useWorkspace(wsId).data;
  const role = w?.my_role;
  // While the role loads, list what every member sees (Approvals, People and Settings wait for the role).
  const altitude = useAltitude({ wsId });
  const counts = useWorkspaceCounts(wsId);
  // (FIX-236, C-1165) Emergency read access: the rail is the fixed read-only set — the topics the
  // mode is allowed to open (C-1164) — because the role is null and no membership decides the rest.
  const topics =
    isEmergencyReader(w)
      ? EMERGENCY_TOPICS
      : visibleTopics("workspace", { instanceAdmin, workspace: role ?? (instanceAdmin ? undefined : "member") }, altitude);
  return (
    <nav aria-label="Workspace">
      <ul className="space-y-0.5">
        {topics.map((t) => {
          // `base` is `/o/<org>/w/<ws>`: the topic's folder under it.
          const href = t === "overview" ? base : `${base}/${t}`;
          return (
            <NavItem
              key={t}
              tour={t}
              href={href}
              label={TOPIC_LABEL[t]}
              icon={TOPIC_ICON[t]!}
              active={topicActive(pathname, href, t)}
              badge={
                t === "approvals" ? (
                  <CountPill n={counts.topics.approvals ?? 0} testId="approvals-badge" />
                ) : t === "overview" ? (
                  <CountPill n={counts.count} testId="overview-badge" />
                ) : undefined
              }
            />
          );
        })}
      </ul>
    </nav>
  );
}

/** (account-4) Profile · Security · Notifications · SSH keys · API keys · Chat platforms · Delete account; SSH keys only while `features.ssh` is on. */
export const ACCOUNT_LINKS: { href: string; label: string; icon: IconType; ssh?: boolean }[] = [
  { href: accountRoutes.profile, label: "Profile", icon: UserIcon },
  { href: accountRoutes.security, label: "Security", icon: LockIcon },
  { href: accountRoutes.notifications, label: "Notifications", icon: BellIcon },
  { href: accountRoutes.ssh, label: "SSH keys", icon: ServerIcon, ssh: true },
  { href: accountRoutes.apiKeys, label: "API keys", icon: KeyIcon },
  { href: accountRoutes.chatPlatforms, label: "Chat platforms", icon: LinkIcon },
  { href: accountRoutes.delete, label: "Delete account", icon: TrashIcon },
];

export function AccountNavList({ pathname }: { pathname: string }) {
  const ssh = useInstance().data?.features?.ssh === true;
  return (
    <nav aria-label="Account">
      <ul className="space-y-0.5">
        {ACCOUNT_LINKS.filter((l) => !l.ssh || ssh).map((l) => (
          <NavItem key={l.href} href={l.href} label={l.label} icon={l.icon} active={pathname === l.href} />
        ))}
      </ul>
    </nav>
  );
}

function AdminNavList({ pathname }: { pathname: string }) {
  const instanceAdmin = useMe().data?.user.is_instance_admin === true;
  const visible = new Set(visibleTopics("admin", { instanceAdmin }));
  // (P5-REFINE-18 #1, rec 9) Overview, then what is decided for everyone, then what belongs to this installation.
  return (
    <GroupedNav
      label="Administration"
      idBase="admin-rail"
      groups={ADMIN_RAIL_GROUPS}
      visible={visible}
      item={(t) => {
        const href = topicHref("admin", t);
        // (P5-REFINE-18 #18, C-1022/C-1080) Overview carries the total: the same count as the sidebar entry and the account menu.
        return (
          <NavItem
            key={t}
            tour={t}
            href={href}
            label={ADMIN_RAIL_LABEL[t]}
            icon={TOPIC_ICON[t]!}
            active={topicActive(pathname, href, t)}
            badge={t === "overview" ? <AttentionBadge /> : <AttentionBadge topic={t} />}
          />
        );
      }}
    />
  );
}

/** (D-508(g)) The rail's foot: Manage <org> of the organization you are in (or came from), with what waits there. */
function RailManage({ place }: { place: Place }) {
  const org = usePickerModel(place, false).current;
  const badge = useOrgBadge(org?.manage ? org.id : undefined);
  if (!org?.manage) return null;
  return (
    <li>
      <Link
        href={orgRoutes(org.slug).home()}
        data-tour="manage"
        className="flex min-h-9 items-center gap-2.5 rounded-control px-2.5 py-1.5 text-sm text-ink-2 hover:bg-subtle hover:text-ink"
      >
        <span aria-hidden="true" className={cn("inline-flex size-4.5 shrink-0 items-center justify-center rounded-full text-micro font-semibold uppercase", tintFor(org.slug))}>
          {org.name.trim().slice(0, 2)}
        </span>{" "}
        <span className="min-w-0 flex-1 truncate">Manage {org.name}</span>{" "}
        <CountBadge n={badge?.count} actionId="manage-switcher-entry" testId="rail-manage-badge" />
      </Link>
    </li>
  );
}

/**
 * (D-508(a)) The rail shows exactly one place — a workspace, its organization (Manage <org>), Administration or Your
 * account — and its top says which: the place card, with a back line outside a workspace.
 */
function SidebarContent({
  orgSlug,
  wsSlug,
  wsId,
  pathname,
}: {
  orgSlug?: string;
  wsSlug?: string;
  wsId?: string;
  pathname: string;
}) {
  const me = useMe();
  const viewer = me.data?.user;
  // The admin navigation is for instance admins only (the page guard already refuses everyone else; the nav must not
  // advertise it to a signed-in member who typed an /admin path, WEBA-11).
  const isInstanceAdmin = viewer?.is_instance_admin === true;
  // Any success may have decided or resolved something: refetch the active counts (nothing is fetched for a query nobody shows).
  useRefreshAttentionOnMutation(true);
  const resolved = useResolve(orgSlug, wsSlug);
  const railWsId = wsSlug ? (wsId ?? resolved.data?.workspace?.id) : undefined;
  const workspace = useWorkspace(railWsId).data;
  // (D-508(h)) A workspace is remembered as the way back only when its pages open for this person: an administrator
  // who is not one of its people is on the way to its manage page (the workspace layout's own rule, FIX-277).
  const opens = !!workspace && !(workspace.my_role == null && workspace.administers === true && !isEmergencyReader(workspace));
  useRecordPlace(
    orgSlug && wsSlug && viewer?.id && workspace && opens
      ? { userId: viewer.id, orgSlug, wsSlug, wsName: workspaceDisplayName(workspace, viewer), reader: isEmergencyReader(workspace) }
      : null,
    pathname,
  );
  const record = usePlaceRecord();
  const inAdmin = pathname.startsWith("/admin");
  const place = placeOf(pathname, orgSlug, wsSlug, isInstanceAdmin);
  const back =
    place.kind === "workspace" ? undefined : backLink(record, viewer?.id, me.data?.memberships, place.kind === "org" ? place.orgSlug : undefined);
  return (
    <div className="flex h-full min-h-0 flex-col gap-4 p-4">
      <Link href="/" className="block self-start rounded-md" aria-label="Aurora home">
        <AuroraLogo className="h-9.5 w-auto" />
      </Link>
      <div className="space-y-1">
        {back ? <PlaceBack back={back} /> : null}
        <ScopeSwitcher place={place} />
      </div>
      <div className="min-h-0 flex-1 overflow-y-auto">
        {place.kind === "admin" ? (
          <AdminNavList pathname={pathname} />
        ) : place.kind === "account" ? (
          <AccountNavList pathname={pathname} />
        ) : place.kind === "org" ? (
          <OrgNavList orgSlug={place.orgSlug} pathname={pathname} />
        ) : orgSlug && wsSlug && !inAdmin ? (
          <WorkspaceNav base={wsRoutes(orgSlug, wsSlug).base} wsId={railWsId} pathname={pathname} />
        ) : null}
      </div>
      <div className="space-y-2 border-t border-line pt-3">
        <ul className="space-y-0.5 empty:hidden">
          {/* (D-508(g), nav-3) Manage <org> for its owners, admins and instance admins; inside it the menu above is it. */}
          {place.kind !== "org" ? <RailManage place={place} /> : null}
          {/* Inside Administration the rail's place card names it: the entry would say it a second time. */}
          {isInstanceAdmin && place.kind !== "admin" ? (
            <NavItem href={adminRoutes.home()} label="Administration" icon={ShieldIcon} active={inAdmin} badge={<AttentionBadge />} tour="administration" />
          ) : null}
        </ul>
        <SearchButton />
        <ThemeSwitch />
        <UserMenu />
      </div>
    </div>
  );
}

/** A 404 from the API: the thing in the address does not exist (or the viewer may not know it does). */
function isNotFound(error: unknown): boolean {
  return error instanceof ApiError && error.status === 404;
}

export function AppShell({ children }: { children: ReactNode }) {
  useUnauthorizedRedirect();
  // (P4-011B-4, C-836) With the local Web Push opt-in marker: re-sync the browser's subscription once per load.
  usePushResync(true);
  // (P5-PERSON, C-1054) An unset personal time zone is filled from the browser once per session.
  useTimezoneAutofill();
  const params = useParams<{ org?: string; ws?: string }>();
  const pathname = usePathname();
  const [navOpen, setNavOpen] = useState(false);
  // The tour opens the drawer itself; then the drawer must not trap focus or close on outside clicks (the tour card is outside it).
  const [tourNav, setTourNav] = useState(false);
  const tourNavOpen = useCallback((open: boolean) => {
    setNavOpen(open);
    setTourNav(open);
  }, []);
  const orgSlug = params.org;
  const wsSlug = params.ws;
  const resolvedQuery = useResolve(orgSlug, wsSlug);
  const resolved = resolvedQuery.data;
  // A workspace is the place only under `/w/`: the manage page (`/o/<org>/workspaces/<ws>`) has a `ws` parameter too,
  // and belongs to Manage <org>.
  const placeWs = /^\/o\/[^/]+\/w\//.test(pathname) ? wsSlug : undefined;
  const wsId = placeWs ? resolved?.workspace?.id : undefined;
  const waiting = useWaitingCount(wsId);
  // (P5-REFINE-2, org-3) An unknown organization or workspace is a 404 *inside* the shell: the rail and switcher show the
  // viewer's real organization, never the unknown slug, and the page body is the not-found state.
  const orgQuery = useResolve(orgSlug);
  const me = useMe();
  const missing: NotFoundKind | null = !orgSlug
    ? null
    : isNotFound(orgQuery.error)
      ? "organization"
      : wsSlug && isNotFound(resolvedQuery.error)
        ? "workspace"
        : null;
  // The rail then shows a real workspace (never the Manage menu, which a member may not see): the viewer's last one,
  // for a missing workspace in the same organization.
  const real = missing ? lastWorkspace(me.data?.user.id, me.data?.memberships, missing === "workspace" ? orgSlug : undefined) : undefined;
  // A real workspace's rail is drawn from its address; with `pathname` outside it nothing records it.
  const side = missing ? { orgSlug: real?.orgSlug, wsSlug: real?.wsSlug, wsId: undefined, pathname } : { orgSlug, wsSlug: placeWs, wsId, pathname };
  // The switcher's menu lives in a portal outside the drawer: close the drawer when the page changes.
  const [seenPath, setSeenPath] = useState(pathname);
  if (seenPath !== pathname) {
    setSeenPath(pathname);
    setNavOpen(false);
  }

  // (FIX-138) The standalone terminal window has no app chrome: it is a pane that fills the browser window.
  if (/^\/o\/[^/]+\/w\/[^/]+\/agents\/[^/]+\/terminal\/?$/.test(pathname)) return <>{children}</>;

  return (
    <div className="min-h-screen md:flex">
      <aside
        aria-label="Sidebar"
        className="sticky top-0 hidden h-screen w-64 shrink-0 border-r border-line bg-surface md:block"
      >
        <SidebarContent {...side} />
      </aside>

      <div className="min-w-0 flex-1">
        {/* (D-508(k), nav-3d) The menu, then the place card in short, which opens the picker as a sheet. */}
        <div className="sticky top-0 z-30 flex items-center gap-2 border-b border-line bg-surface px-3 py-2 md:hidden">
          <DialogPrimitive.Root open={navOpen} onOpenChange={setNavOpen} modal={!tourNav}>
            <DialogPrimitive.Trigger asChild>
              <button
                type="button"
                aria-label={waiting > 0 ? `Open menu, ${attentionText(waiting)}` : "Open menu"}
                className="relative inline-flex size-11 shrink-0 items-center justify-center rounded-control text-ink hover:bg-subtle"
              >
                <MenuIcon className="size-5" />
                {waiting > 0 ? (
                  <span data-testid="menu-dot" className="absolute right-2 top-2 size-2 rounded-full bg-pending-ink" aria-hidden="true" />
                ) : null}
              </button>
            </DialogPrimitive.Trigger>
            <DialogPrimitive.Portal>
              <DialogPrimitive.Overlay className="fixed inset-0 z-40 bg-scrim" />
              <DialogPrimitive.Content
                // It opens from the side its button is on.
                className="fixed inset-y-0 left-0 z-50 w-[min(18rem,calc(100vw-3rem))] border-r border-line bg-surface text-ink shadow-pop"
                onInteractOutside={(e) => {
                  if (tourNav) e.preventDefault();
                }}
                // Close the drawer after following a link.
                onClick={(e) => {
                  if ((e.target as HTMLElement).closest("a[href]")) setNavOpen(false);
                }}
              >
                <DialogPrimitive.Title className="sr-only">Navigation</DialogPrimitive.Title>
                <DialogPrimitive.Description className="sr-only">Site navigation menu</DialogPrimitive.Description>
                <DialogPrimitive.Close asChild>
                  <button
                    type="button"
                    aria-label="Close menu"
                    className="absolute right-2 top-2 inline-flex size-11 items-center justify-center rounded-control text-ink-2 hover:bg-subtle"
                  >
                    <XIcon className="size-5" />
                  </button>
                </DialogPrimitive.Close>
                <SidebarContent {...side} />
              </DialogPrimitive.Content>
            </DialogPrimitive.Portal>
          </DialogPrimitive.Root>
          <TopBarPlace place={placeOf(side.pathname, side.orgSlug, side.wsSlug, me.data?.user.is_instance_admin === true)} />
        </div>
        {/* No max width here: each page sets its own (docs/14 page widths), so the agent page gets the full content width. */}
        <main className="w-full min-w-0 space-y-4 px-4 py-6 md:px-8 md:py-8">
          <ApprovalNotifier />
          <FrozenEntityBanner />
          <EmergencyBanners />
          {missing ? <NotFoundState kind={missing} slug={missing === "organization" ? orgSlug : wsSlug} /> : children}
        </main>
        <Tour setNavOpen={tourNavOpen} />
        <CommandPalette />
        <ShortcutsHost />
        <UndoToastHost />
      </div>
    </div>
  );
}
