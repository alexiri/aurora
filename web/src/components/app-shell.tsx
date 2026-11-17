"use client";

import * as DialogPrimitive from "@radix-ui/react-dialog";
import Link from "next/link";
import { useParams, usePathname, useRouter } from "next/navigation";
import { useCallback, useId, useState, type ComponentType, type ReactNode, type SVGProps } from "react";

import { usePendingApprovals } from "@/lib/api/approvals";
import { clearDrafts } from "@/components/chat/composer/drafts";
import { logout, useMe, useOrg, useOrgs, useResolve, useWorkspace, useWorkspaces } from "@/lib/api/queries";
import { visibleTopics } from "@/lib/api/roles";
import { loginUrl, useUnauthorizedRedirect } from "@/lib/auth/use-unauthorized-redirect";
import { showWhatMoved } from "@/lib/moved";
import { removeThisDevicePush, usePushResync } from "@/lib/web-push";
import { accountRoutes, adminRoutes, TOPIC_LABEL, topicHref, orgRoutes, wsRoutes } from "@/lib/routes";
import { cn } from "@/lib/utils";

import { ApprovalNotifier } from "./approvals/approval-notifier";
import { FrozenEntityBanner } from "./freeze/frozen-entity-banner";
import { RenamesNotice } from "./onboarding/renames-notice";
import { Tour } from "./onboarding/tour";
import { startTour } from "./onboarding/tour-events";
import { ThemeSwitch } from "./theme/theme-switch";
import { Avatar, tintFor } from "./agent-avatar";
import { AuroraLogo } from "./ui/aurora-logo";
import {
  ActivityIcon,
  BotIcon,
  BookIcon,
  BuildingIcon,
  CheckCircleIcon,
  ChevronsUpDownIcon,
  HomeIcon,
  KeyIcon,
  LinkIcon,
  LockIcon,
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
import { Menu, MenuContent, MenuItem, MenuLabel, MenuTrigger } from "./ui/menu";
import { PageLevelProvider, type PageLevel } from "./ui/page-header";

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

/**
 * FIX-027: one sidebar select control (an uppercase caption above a one-line menu trigger).
 * The caption is the control's label (`aria-labelledby` names the caption, the avatar's
 * initial and the value, so the accessible name contains every visible token — WCAG 2.5.3);
 * there is no aria-label.
 */
function SidebarSelect({
  caption,
  placeholder,
  value,
  avatar,
  testId,
  children,
}: {
  caption: string;
  /** Shown when nothing is the current scope. */
  placeholder: string;
  /** The current scope's name, or undefined for the placeholder look. */
  value?: string;
  avatar?: { name: string; tintKey?: string };
  testId: string;
  children: ReactNode;
}) {
  const id = useId();
  return (
    <div data-testid={testId} className="space-y-1.5">
      <p id={`${id}-caption`} className="px-1 text-label font-semibold uppercase tracking-label text-muted">
        {caption}
      </p>
      <Menu>
        <MenuTrigger asChild>
          <button
            type="button"
            // The accessible name names the caption, the avatar's initial and the value, so it
            // contains every visible token (WCAG 2.5.3, axe `label-content-name-mismatch` —
            // the initial chip is visible on screen, so the name must include it). The
            // placeholder state has no name yet: its "?" chip stays out of the name.
            aria-labelledby={
              avatar && value !== undefined
                ? `${id}-caption ${id}-avatar ${id}-value`
                : `${id}-caption ${id}-value`
            }
            className="flex min-h-11 w-full items-center gap-2.5 rounded-control border border-line bg-surface px-2.5 py-1.5 text-left hover:bg-subtle"
          >
            {avatar && value !== undefined ? (
              <span
                id={`${id}-avatar`}
                className={cn(
                  "inline-flex size-7 shrink-0 items-center justify-center rounded-full text-xs font-semibold uppercase",
                  tintFor(avatar.tintKey ?? avatar.name),
                )}
              >
                {avatar.name.trim().charAt(0) || "?"}
              </span>
            ) : null}
            {/* The explicit space keeps the chip's initial a separate visible token (JSX
                would otherwise fuse "A" + "Acme" into "AAcme"); flexbox drops whitespace-only
                runs, so the visual layout is unchanged. */}
            {avatar && value !== undefined ? " " : null}
            <span
              id={`${id}-value`}
              className="min-w-0 flex-1 truncate text-sm font-semibold text-ink"
            >
              {value ?? placeholder}
            </span>
            <ChevronsUpDownIcon className="size-4 shrink-0 text-muted" />
          </button>
        </MenuTrigger>
        <MenuContent className="w-[var(--radix-dropdown-menu-trigger-width)] min-w-60">{children}</MenuContent>
      </Menu>
    </div>
  );
}

/** FIX-027: organizations on every page. It lists organizations only (C-962): Administration has its own entry. */
function OrgSelector({ orgSlug }: { orgSlug?: string }) {
  const orgs = useOrgs();
  const resolved = useResolve(orgSlug);
  const org = orgs.data?.items.find((o) => o.slug === orgSlug);
  // /v1/resolve also names an org the person has no membership row for (an admin visiting it).
  const orgName = org?.name ?? resolved.data?.org.name ?? orgSlug;
  return (
    <SidebarSelect
      caption="Organization"
      placeholder="Select organization"
      value={orgName}
      avatar={{ name: orgName ?? "?", tintKey: orgSlug }}
      testId="org-selector"
    >
      <MenuLabel>Organizations</MenuLabel>
      {orgs.isPending ? <MenuItem disabled>Loading…</MenuItem> : null}
      {orgs.isError ? <MenuItem disabled>Could not load organizations</MenuItem> : null}
      {orgs.data?.items.map((o) => (
        <MenuItem key={o.id} asChild>
          <Link href={orgRoutes(o.slug).home()} className={cn(o.slug === orgSlug && "font-semibold")}>
            {o.name}
          </Link>
        </MenuItem>
      ))}
    </SidebarSelect>
  );
}

/** FIX-027: the current organization's workspaces — rendered only where an organization is active. */
function WorkspaceSelector({ orgSlug, wsSlug, pathname }: { orgSlug: string; wsSlug?: string; pathname: string }) {
  const resolved = useResolve(orgSlug);
  const workspaces = useWorkspaces(resolved.data?.org.id);
  const ws = workspaces.data?.items.find((w) => w.slug === wsSlug);
  const atRoot = !pathname.startsWith(`${orgRoutes(orgSlug).base}/w/`);
  return (
    <SidebarSelect
      caption="Workspace"
      placeholder={atRoot ? "All workspaces" : (wsSlug ?? "?")}
      value={atRoot ? "All workspaces" : ws?.name}
      testId="workspace-selector"
    >
      <MenuLabel>Workspaces</MenuLabel>
      {workspaces.isPending ? <MenuItem disabled>Loading…</MenuItem> : null}
      {workspaces.isError ? <MenuItem disabled>Could not load workspaces</MenuItem> : null}
      {workspaces.data?.items.length === 0 ? <MenuItem disabled>No workspaces</MenuItem> : null}
      {workspaces.data?.items.map((w) => (
        <MenuItem key={w.id} asChild>
          <Link href={wsRoutes(orgSlug, w.slug).home()} className={cn(w.slug === wsSlug && "font-semibold")}>
            {w.name}
          </Link>
        </MenuItem>
      ))}
      <MenuItem asChild>
        <Link
          href={orgRoutes(orgSlug).home()}
          data-active={atRoot ? "true" : undefined}
          className={cn(atRoot && "font-semibold")}
        >
          All workspaces…
        </Link>
      </MenuItem>
    </SidebarSelect>
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
  return (
    <Menu>
      <MenuTrigger asChild>
        <button
          type="button"
          className="flex min-h-11 w-full items-center gap-2.5 rounded-control px-2.5 py-1.5 text-left hover:bg-subtle"
          // No aria-label: the name comes from the visible text (WCAG 2.5.3, see the sidebar selects).
        >
          <Avatar name={name ?? "?"} tintKey={me.data?.user.email} size={28} />
          <span className="min-w-0 flex-1">
            <span className="block truncate text-sm font-semibold text-ink">{name ?? "Account"}</span>
            {me.data?.user.is_instance_admin ? <span className="block text-xs text-muted">Instance admin</span> : null}
          </span>
          <ChevronsUpDownIcon className="size-4 shrink-0 text-muted" />
        </button>
      </MenuTrigger>
      <MenuContent align="start" side="top" className="w-[var(--radix-dropdown-menu-trigger-width)] min-w-56">
        {me.data ? <MenuLabel>{me.data.user.email}</MenuLabel> : null}
        <MenuItem asChild>
          <Link href={accountRoutes.profile}>Your account</Link>
        </MenuItem>
        {/* C-962: Administration is a labelled entry, here and at the foot of the sidebar, for instance admins. */}
        {me.data?.user.is_instance_admin === true ? (
          <MenuItem asChild>
            <Link href={adminRoutes.home()}>Administration</Link>
          </MenuItem>
        ) : null}
        <MenuItem onSelect={onTour}>Show me around</MenuItem>
        <MenuItem onSelect={showWhatMoved}>What moved</MenuItem>
        <MenuItem onSelect={() => void onLogout()}>Sign out</MenuItem>
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
  return (
    <div className="flex h-full min-h-0 flex-col gap-4 p-4">
      <Link href="/" className="block self-start rounded-md" aria-label="Aurora home">
        <AuroraLogo className="h-9.5 w-auto" />
      </Link>
      {/* FIX-027: two stacked selects on every page — organization, then (only with an active
          organization) its workspaces. Administration is its own entry in the footer (C-962). */}
      <OrgSelector orgSlug={orgSlug} />
      {orgSlug ? <WorkspaceSelector orgSlug={orgSlug} wsSlug={wsSlug} pathname={pathname} /> : null}
      <div className="min-h-0 flex-1 overflow-y-auto">
        {base ? (
          <WorkspaceNav base={base} wsId={wsId} pathname={pathname} />
        ) : orgSlug ? (
          <OrgNav orgSlug={orgSlug} pathname={pathname} />
        ) : inAdmin && isInstanceAdmin ? (
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

export function AppShell({ children }: { children: ReactNode }) {
  useUnauthorizedRedirect();
  // (P4-011B-4, C-836) With the local Web Push opt-in marker: re-sync the browser's subscription once per load.
  usePushResync(true);
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
  const resolved = useResolve(orgSlug, wsSlug).data;
  const wsId = resolved?.workspace?.id;
  const side = { orgSlug, wsSlug, wsId, pathname };
  // C-962: the level badge every page header carries (Administration, <org> or <org> › <workspace>).
  const orgName = resolved?.org.name ?? orgSlug;
  const level: PageLevel | null = pathname.startsWith("/admin")
    ? { kind: "instance", label: "Administration" }
    : orgSlug && wsSlug
      ? { kind: "workspace", label: `${orgName} › ${resolved?.workspace?.name ?? wsSlug}` }
      : orgSlug
        ? { kind: "org", label: orgName ?? orgSlug }
        : null;

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
          <PageLevelProvider value={level}>{children}</PageLevelProvider>
        </main>
        <Tour setNavOpen={tourNavOpen} />
      </div>
    </div>
  );
}
