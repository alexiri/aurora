"use client";

import * as DialogPrimitive from "@radix-ui/react-dialog";
import Link from "next/link";
import { useParams, usePathname, useRouter } from "next/navigation";
import { useCallback, useEffect, useMemo, useState, useSyncExternalStore, type ComponentType, type ReactNode, type SVGProps } from "react";

import { usePendingApprovals } from "@/lib/api/approvals";
import { clearDrafts } from "@/components/chat/composer/drafts";
import { logout, useMe, useOrg, useResolve, useWorkspace } from "@/lib/api/queries";
import { visibleTopics } from "@/lib/api/roles";
import { loginUrl, useUnauthorizedRedirect } from "@/lib/auth/use-unauthorized-redirect";
import { ApiError } from "@/lib/api/client";
import { lastScope } from "@/lib/nav/recent-scopes";
import { showWhatMoved } from "@/lib/moved";
import { removeThisDevicePush, usePushResync } from "@/lib/web-push";
import { accountRoutes, adminRoutes, TOPIC_LABEL, topicHref, wsRoutes } from "@/lib/routes";
import { useTimezoneAutofill } from "@/lib/timezone";
import { cn } from "@/lib/utils";

import { AttentionBadge } from "./admin/attention-badge";
import { ApprovalNotifier } from "./approvals/approval-notifier";
import { FrozenEntityBanner } from "./freeze/frozen-entity-banner";
import { ScopeSwitcher } from "./nav/scope-switcher";
import { RenamesNotice } from "./onboarding/renames-notice";
import { Tour } from "./onboarding/tour";
import { startTour } from "./onboarding/tour-events";
import { ThemeSwitch } from "./theme/theme-switch";
import { Avatar } from "./agent-avatar";
import { NotFoundState, type NotFoundKind } from "./not-found-state";
import { AuroraLogo } from "./ui/aurora-logo";
import { UndoToastHost } from "./ui/undo-toast";
import {
  ActivityIcon,
  ArrowLeftIcon,
  BotIcon,
  BookIcon,
  BuildingIcon,
  CheckCircleIcon,
  ChevronsUpDownIcon,
  HomeIcon,
  KeyIcon,
  LinkIcon,
  LockIcon,
  LogOutIcon,
  MenuIcon,
  MessageIcon,
  PlugIcon,
  ScaleIcon,
  ScrollIcon,
  ServerIcon,
  SettingsIcon,
  ShieldIcon,
  TrashIcon,
  UserIcon,
  UsersIcon,
  XIcon,
} from "./ui/icons";
import { Menu, MenuContent, MenuItem, MenuSeparator, MenuTrigger } from "./ui/menu";

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

function UserMenu() {
  const me = useMe();
  const router = useRouter();
  const name = me.data?.user.display_name;
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
            <Avatar name={me.data.user.display_name} tintKey={me.data.user.email} size={32} />
            <div className="min-w-0">
              <p className="truncate text-sm font-semibold text-ink">{me.data.user.display_name}</p>
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
        <MenuItem onSelect={onTour} className={itemClass}>
          <HomeIcon className="size-4 shrink-0 text-muted" />
          Show me around
        </MenuItem>
        <MenuItem onSelect={showWhatMoved} className={itemClass}>
          <ScrollIcon className="size-4 shrink-0 text-muted" />
          What moved
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
  settings: SettingsIcon,
};

/** Overview and Workspaces are the level's own address; every other topic owns the paths below its folder. */
function topicActive(pathname: string, href: string, topic: string): boolean {
  const path = href.split("?")[0]!;
  return topic === "overview" || topic === "workspaces" ? pathname === path : pathname === path || pathname.startsWith(`${path}/`);
}

function OrgNav({ orgSlug, pathname }: { orgSlug: string; pathname: string }) {
  const resolved = useResolve(orgSlug);
  const org = useOrg(resolved.data?.org.id);
  const instanceAdmin = useMe().data?.user.is_instance_admin === true;
  // While the role loads, list what every member sees; the pages refuse nothing they list.
  const topics = visibleTopics("org", { instanceAdmin, org: org.data?.my_role ?? (instanceAdmin ? undefined : "member") });
  return (
    <nav aria-label="Organization">
      <ul className="space-y-0.5">
        {topics.map((t) => {
          const href = topicHref("org", t, orgSlug);
          return <NavItem key={t} tour={t} href={href} label={TOPIC_LABEL[t]} icon={TOPIC_ICON[t]!} active={topicActive(pathname, href, t)} />;
        })}
      </ul>
    </nav>
  );
}

/** Pending-count pill; the query only runs where the approvals page exists (RFE-012). */
function PendingPill({ wsId }: { wsId: string }) {
  const pending = usePendingApprovals(wsId);
  const n = pending.data?.items.length ?? 0;
  if (n === 0) return null;
  return (
    <span className="rounded-full bg-pending-soft px-1.5 text-label font-semibold leading-5 tabular-nums text-pending-ink">
      {n}
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
  const role = useWorkspace(wsId).data?.my_role;
  // While the role loads, list what every member sees (Approvals, People and Settings wait for the role).
  const topics = visibleTopics("workspace", { instanceAdmin, workspace: role ?? (instanceAdmin ? undefined : "member") });
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
              badge={t === "approvals" && wsId ? <PendingPill wsId={wsId} /> : undefined}
            />
          );
        })}
      </ul>
    </nav>
  );
}

const ACCOUNT_LINKS: { href: string; label: string; icon: IconType }[] = [
  { href: accountRoutes.profile, label: "Profile", icon: UserIcon },
  { href: accountRoutes.security, label: "Security", icon: LockIcon },
  { href: accountRoutes.apiKeys, label: "API keys", icon: KeyIcon },
  { href: accountRoutes.chatPlatforms, label: "Chat platforms", icon: LinkIcon },
  { href: accountRoutes.delete, label: "Delete account", icon: TrashIcon },
];

function AccountNavList({ pathname }: { pathname: string }) {
  return (
    <nav aria-label="Account">
      <ul className="space-y-0.5">
        {ACCOUNT_LINKS.map((l) => (
          <NavItem key={l.href} href={l.href} label={l.label} icon={l.icon} active={pathname === l.href} />
        ))}
      </ul>
    </nav>
  );
}

function AdminNavList({ pathname }: { pathname: string }) {
  const instanceAdmin = useMe().data?.user.is_instance_admin === true;
  return (
    <nav aria-label="Administration">
      <ul className="space-y-0.5">
        {visibleTopics("admin", { instanceAdmin }).map((t) => {
          const href = topicHref("admin", t);
          return <NavItem key={t} tour={t} href={href} label={TOPIC_LABEL[t]} icon={TOPIC_ICON[t]!} active={topicActive(pathname, href, t)} />;
        })}
      </ul>
    </nav>
  );
}

/** Where Administration's "Back to <Org>" goes: the organization and workspace you came from (this tab only). */
const RETURN_KEY = "aurora.admin-return";
interface ReturnTo {
  href: string;
  name: string;
}
const noopSubscribe = () => () => {};
function readRawReturn(): string | null {
  try {
    return window.sessionStorage.getItem(RETURN_KEY);
  } catch {
    return null;
  }
}
/** An internal path only: starts with one "/", no scheme, no protocol-relative "//", no backslash. */
export function isInternalPath(href: string): boolean {
  return href.startsWith("/") && !href.startsWith("//") && !href.includes("\\") && !/^[a-z][a-z0-9+.-]*:/i.test(href);
}

export function parseReturn(raw: string | null): ReturnTo | null {
  try {
    const v: unknown = raw ? JSON.parse(raw) : null;
    if (v && typeof v === "object" && "href" in v && "name" in v && typeof v.href === "string" && typeof v.name === "string" && isInternalPath(v.href)) {
      return { href: v.href, name: v.name };
    }
  } catch {
    // Storage is blocked: fall back to the first workspace.
  }
  return null;
}

/** Remember the organization page the person is on, so Administration can send them back to it. */
function useRememberReturn(orgSlug: string | undefined, wsSlug: string | undefined, orgName: string | undefined, active: boolean) {
  useEffect(() => {
    if (!active || !orgSlug || !orgName) return;
    try {
      const href = wsSlug ? wsRoutes(orgSlug, wsSlug).home() : `/o/${orgSlug}`;
      window.sessionStorage.setItem(RETURN_KEY, JSON.stringify({ href, name: orgName }));
    } catch {
      // Storage is blocked: nothing is remembered.
    }
  }, [active, orgSlug, wsSlug, orgName]);
}

/** E8: Administration's rail — back to where you came from, the level header, the admin topics; no switcher. */
function AdminRailHeader() {
  const me = useMe();
  // Session storage is the external store (null on the server, so the first render matches).
  const raw = useSyncExternalStore(noopSubscribe, readRawReturn, () => null);
  const stored = useMemo(() => parseReturn(raw), [raw]);
  const first = me.data?.memberships?.find((m) => m.workspaces.length > 0);
  const fallback: ReturnTo | null = first
    ? { href: wsRoutes(first.org.slug, first.workspaces[0]!.slug).home(), name: first.org.name ?? first.org.slug }
    : null;
  const back = stored ?? fallback ?? { href: "/", name: "Aurora" };
  return (
    <div className="space-y-3">
      <Link href={back.href} className="flex min-h-9 items-center gap-2 rounded-control px-2.5 py-1.5 text-sm text-ink-2 hover:bg-subtle hover:text-ink">
        <ArrowLeftIcon className="size-4 shrink-0 text-muted" />
        <span className="min-w-0 truncate">Back to {back.name}</span>
      </Link>
      <div className="px-2.5">
        <p className="text-label font-semibold uppercase tracking-label text-muted">You&apos;re in</p>
        <p className="font-display text-base font-semibold text-ink">Administration</p>
      </div>
    </div>
  );
}

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
  const base = orgSlug && wsSlug ? wsRoutes(orgSlug, wsSlug).base : null;
  const inAdmin = pathname.startsWith("/admin");
  // The admin navigation is for instance admins only (the page guard already refuses everyone else; the nav must not
  // advertise it to a signed-in member who typed an /admin path, WEBA-11).
  const isInstanceAdmin = useMe().data?.user.is_instance_admin === true;
  const resolved = useResolve(orgSlug);
  useRememberReturn(orgSlug, wsSlug, resolved.data?.org.name, !inAdmin);
  const adminRail = inAdmin && isInstanceAdmin;
  return (
    <div className="flex h-full min-h-0 flex-col gap-4 p-4">
      <Link href="/" className="block self-start rounded-md" aria-label="Aurora home">
        <AuroraLogo className="h-9.5 w-auto" />
      </Link>
      {/* D-389: one switcher says where you are (workspace, then organization). Administration has no switcher: its own header. */}
      {adminRail ? <AdminRailHeader /> : <ScopeSwitcher orgSlug={orgSlug} wsSlug={wsSlug} />}
      <div className="min-h-0 flex-1 overflow-y-auto">
        {base ? (
          <WorkspaceNav base={base} wsId={wsId} pathname={pathname} />
        ) : orgSlug ? (
          <OrgNav orgSlug={orgSlug} pathname={pathname} />
        ) : adminRail ? (
          <AdminNavList pathname={pathname} />
        ) : pathname.startsWith("/account") ? (
          <AccountNavList pathname={pathname} />
        ) : null}
      </div>
      <div className="space-y-2 border-t border-line pt-3">
        {isInstanceAdmin ? (
          <ul>
            <NavItem
              href={adminRoutes.home()}
              label="Administration"
              icon={ShieldIcon}
              active={inAdmin}
              badge={<AttentionBadge />}
              tour="administration"
            />
          </ul>
        ) : null}
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
  const wsId = resolved?.workspace?.id;
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
  const real = missing === "organization" ? lastScope(me.data?.user.id, me.data?.memberships) : undefined;
  const side =
    missing === "organization"
      ? { orgSlug: real?.orgSlug, wsSlug: undefined, wsId: undefined, pathname }
      : missing === "workspace"
        ? { orgSlug, wsSlug: undefined, wsId: undefined, pathname }
        : { orgSlug, wsSlug, wsId, pathname };
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
        <div className="sticky top-0 z-30 flex items-center justify-between border-b border-line bg-surface px-4 py-2 md:hidden">
          <Link href="/" aria-label="Aurora home" className="rounded-md">
            <AuroraLogo className="h-8 w-auto" />
          </Link>
          <DialogPrimitive.Root open={navOpen} onOpenChange={setNavOpen} modal={!tourNav}>
            <DialogPrimitive.Trigger asChild>
              <button
                type="button"
                aria-label="Open menu"
                className="inline-flex size-11 items-center justify-center rounded-control text-ink hover:bg-subtle"
              >
                <MenuIcon className="size-5" />
              </button>
            </DialogPrimitive.Trigger>
            <DialogPrimitive.Portal>
              <DialogPrimitive.Overlay className="fixed inset-0 z-40 bg-scrim" />
              <DialogPrimitive.Content
                className="fixed inset-y-0 right-0 z-50 w-[min(18rem,calc(100vw-3rem))] border-l border-line bg-surface text-ink shadow-pop"
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
        </div>
        {/* No max width here: each page sets its own (docs/14 page widths), so the agent page gets the full content width. */}
        <main className="w-full min-w-0 space-y-4 px-4 py-6 md:px-8 md:py-8">
          <ApprovalNotifier />
          <FrozenEntityBanner />
          <RenamesNotice />
          {missing ? <NotFoundState kind={missing} slug={missing === "organization" ? orgSlug : wsSlug} /> : children}
        </main>
        <Tour setNavOpen={tourNavOpen} />
        <UndoToastHost />
      </div>
    </div>
  );
}
