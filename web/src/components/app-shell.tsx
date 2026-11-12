"use client";

import * as DialogPrimitive from "@radix-ui/react-dialog";
import Link from "next/link";
import { useParams, usePathname, useRouter } from "next/navigation";
import { useCallback, useId, useState, type ComponentType, type ReactNode, type SVGProps } from "react";

import { useCanApprove, usePendingApprovals } from "@/lib/api/approvals";
import { logout, useMe, useOrg, useOrgs, useResolve, useWorkspaces } from "@/lib/api/queries";
import { loginUrl, useUnauthorizedRedirect } from "@/lib/auth/use-unauthorized-redirect";
import { cn } from "@/lib/utils";

import { ApprovalNotifier } from "./approvals/approval-notifier";
import { FrozenEntityBanner } from "./freeze/frozen-entity-banner";
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
import { Menu, MenuContent, MenuItem, MenuLabel, MenuSeparator, MenuTrigger } from "./ui/menu";

type IconType = ComponentType<SVGProps<SVGSVGElement>>;

function NavItem({
  href,
  label,
  active,
  icon: Icon,
  badge,
}: {
  href: string;
  label: string;
  active: boolean;
  icon: IconType;
  badge?: ReactNode;
}) {
  return (
    <li>
      <Link
        href={href}
        aria-current={active ? "page" : undefined}
        data-tour={label.toLowerCase().replace(/\s+/g, "-")}
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
  indent,
  testId,
  children,
}: {
  caption: string;
  /** Shown when nothing is the current scope. */
  placeholder: string;
  /** The current scope's name, or undefined for the placeholder look. */
  value?: string;
  avatar?: { name: string; tintKey?: string };
  /** The workspace control sits inside its organization (FIX-027). */
  indent?: boolean;
  testId: string;
  children: ReactNode;
}) {
  const id = useId();
  return (
    <div data-testid={testId} className={cn("space-y-1.5", indent && "pl-8")}>
      <p id={`${id}-caption`} className="px-1 text-[11px] font-semibold uppercase tracking-[0.06em] text-muted">
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

/** FIX-027: organizations on every page; instance admins reach the admin area only from here. */
function OrgSelector({ orgSlug, pathname }: { orgSlug?: string; pathname: string }) {
  const me = useMe();
  const orgs = useOrgs();
  const resolved = useResolve(orgSlug);
  const isAdmin = me.data?.user.is_instance_admin === true;
  const inAdmin = pathname.startsWith("/admin");
  const org = orgs.data?.items.find((o) => o.slug === orgSlug);
  // /v1/resolve also names an org the person has no membership row for (an admin visiting it).
  const orgName = org?.name ?? resolved.data?.org.name ?? orgSlug;
  return (
    <SidebarSelect
      caption="Organization"
      placeholder={inAdmin ? "All organizations" : "Select organization"}
      value={orgName}
      avatar={{ name: orgName ?? "?", tintKey: orgSlug }}
      testId="org-selector"
    >
      <MenuLabel>Organizations</MenuLabel>
      {orgs.isPending ? <MenuItem disabled>Loading…</MenuItem> : null}
      {orgs.isError ? <MenuItem disabled>Could not load organizations</MenuItem> : null}
      {orgs.data?.items.map((o) => (
        <MenuItem key={o.id} asChild>
          <Link href={`/o/${o.slug}`} className={cn(o.slug === orgSlug && "font-semibold")}>
            {o.name}
          </Link>
        </MenuItem>
      ))}
      {isAdmin ? (
        <>
          <MenuSeparator />
          <MenuItem asChild>
            <Link href="/admin" data-active={inAdmin ? "true" : undefined} className={cn(inAdmin && "font-semibold")}>
              <ShieldIcon className="mr-2 inline size-4 text-muted" />
              All organizations
            </Link>
          </MenuItem>
        </>
      ) : null}
    </SidebarSelect>
  );
}

/** FIX-027: the current organization's workspaces — rendered only where an organization is active. */
function WorkspaceSelector({ orgSlug, wsSlug, pathname }: { orgSlug: string; wsSlug?: string; pathname: string }) {
  const resolved = useResolve(orgSlug);
  const workspaces = useWorkspaces(resolved.data?.org.id);
  const ws = workspaces.data?.items.find((w) => w.slug === wsSlug);
  const atRoot = !pathname.startsWith(`/o/${orgSlug}/w/`);
  return (
    <SidebarSelect
      caption="Workspace"
      placeholder={atRoot ? "All workspaces" : (wsSlug ?? "?")}
      value={atRoot ? "All workspaces" : ws?.name}
      indent
      testId="workspace-selector"
    >
      <MenuLabel>Workspaces</MenuLabel>
      {workspaces.isPending ? <MenuItem disabled>Loading…</MenuItem> : null}
      {workspaces.isError ? <MenuItem disabled>Could not load workspaces</MenuItem> : null}
      {workspaces.data?.items.length === 0 ? <MenuItem disabled>No workspaces</MenuItem> : null}
      {workspaces.data?.items.map((w) => (
        <MenuItem key={w.id} asChild>
          <Link href={`/o/${orgSlug}/w/${w.slug}`} className={cn(w.slug === wsSlug && "font-semibold")}>
            {w.name}
          </Link>
        </MenuItem>
      ))}
      <MenuItem asChild>
        <Link
          href={`/o/${orgSlug}`}
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
    try {
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
          <Link href="/account">Account</Link>
        </MenuItem>
        {/* FIX-027: no Admin item here — instance admins use the sidebar's organization selector ("All organizations"). */}
        <MenuItem onSelect={onTour}>Show me around</MenuItem>
        <MenuItem onSelect={() => void onLogout()}>Sign out</MenuItem>
      </MenuContent>
    </Menu>
  );
}

const ORG_LINKS: { path: string; label: string; icon: IconType }[] = [
  { path: "policy", label: "Policy", icon: ShieldIcon },
  { path: "skills", label: "Skills", icon: BookIcon },
  { path: "members", label: "Members", icon: UsersIcon },
  { path: "groups", label: "Groups", icon: UsersIcon },
  // (P4-GRP-2) Owners only: the SCIM directory sync page lives under /settings/directory.
  { path: "settings/directory", label: "Directory sync", icon: KeyIcon },
  { path: "channels", label: "Channels", icon: MessageIcon },
  { path: "instances", label: "Instances", icon: ServerIcon },
  { path: "api-keys", label: "API keys", icon: KeyIcon },
  { path: "audit", label: "Audit log", icon: ScrollIcon },
  { path: "storage", label: "Storage", icon: ServerIcon },
  { path: "settings", label: "Settings", icon: SettingsIcon },
];

function OrgNav({ orgSlug, pathname }: { orgSlug: string; pathname: string }) {
  const resolved = useResolve(orgSlug);
  const org = useOrg(resolved.data?.org.id);
  const role = org.data?.my_role;
  const admin = role === "owner" || role === "admin";
  const base = `/o/${orgSlug}`;
  const item = (href: string, label: string, icon: IconType, active: boolean) => (
    <NavItem key={href} href={href} label={label} icon={icon} active={active} />
  );
  return (
    <nav aria-label="Organization">
      <ul className="space-y-0.5">
        {item(base, "Workspaces", BuildingIcon, pathname === base)}
        {item(`${base}/credentials`, "Credentials", KeyIcon, pathname.startsWith(`${base}/credentials`))}
        {item(`${base}/apps`, "Apps", PlugIcon, pathname.startsWith(`${base}/apps`))}
        {item(`${base}/governance`, "Governance", ScaleIcon, pathname.startsWith(`${base}/governance`))}
        {admin ? ORG_LINKS.map((l) => item(`${base}/${l.path}`, l.label, l.icon, pathname.startsWith(`${base}/${l.path}`))) : null}
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
    <span className="rounded-full bg-pending-soft px-1.5 text-[11px] font-semibold leading-5 tabular-nums text-pending-ink">
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
  // Phase 2, RFE-012: the approvals page exists only for `can_approve` users.
  const canApprove = useCanApprove(wsId);
  const item = (href: string, label: string, icon: IconType, active: boolean, badge?: ReactNode) => (
    <NavItem key={href} href={href} label={label} icon={icon} active={active} badge={badge} />
  );
  return (
    <nav aria-label="Workspace">
      <ul className="space-y-0.5">
        {item(base, "Overview", HomeIcon, pathname === base)}
        {item(`${base}/agents`, "Agents", BotIcon, pathname.startsWith(`${base}/agents`))}
        {item(`${base}/credentials`, "Credentials", KeyIcon, pathname.startsWith(`${base}/credentials`))}
        {item(`${base}/policy`, "Policy", ShieldIcon, pathname.startsWith(`${base}/policy`))}
        {item(`${base}/skills`, "Skills", BookIcon, pathname.startsWith(`${base}/skills`))}
        {canApprove && wsId
          ? item(`${base}/approvals`, "Approvals", CheckCircleIcon, pathname.startsWith(`${base}/approvals`), <PendingPill wsId={wsId} />)
          : null}
        {item(`${base}/activity`, "Activity", ActivityIcon, pathname.startsWith(`${base}/activity`))}
        {/* FIX-027: no Members item — it jumped to the org's members view. Workspace people live in Settings › Access. */}
        {item(`${base}/settings`, "Settings", SettingsIcon, pathname.startsWith(`${base}/settings`))}
      </ul>
    </nav>
  );
}

const ACCOUNT_LINKS: { href: string; label: string; icon: IconType }[] = [
  { href: "/account", label: "Profile", icon: UserIcon },
  { href: "/account/security", label: "Security", icon: LockIcon },
  { href: "/account/api-keys", label: "API keys", icon: KeyIcon },
  { href: "/account/channels", label: "Channels", icon: LinkIcon },
  { href: "/account/delete", label: "Delete account", icon: TrashIcon },
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

const ADMIN_LINKS: { href: string; label: string; icon: IconType }[] = [
  { href: "/admin/setup", label: "Setup", icon: CheckCircleIcon },
  { href: "/admin/users", label: "Users", icon: UsersIcon },
  { href: "/admin/orgs", label: "Organizations", icon: BuildingIcon },
  { href: "/admin/identity", label: "Identity", icon: ShieldIcon },
  { href: "/admin/placement", label: "Sign-up & placement", icon: UsersIcon },
  { href: "/admin/plugins", label: "Plugins", icon: PlugIcon },
  { href: "/admin/connectors", label: "Connectors", icon: LinkIcon },
  { href: "/admin/runners", label: "Runners", icon: ServerIcon },
  { href: "/admin/storage", label: "Storage", icon: ServerIcon },
  { href: "/admin/host-claims", label: "Host claims", icon: ServerIcon },
  { href: "/admin/instances", label: "Instances", icon: ServerIcon },
  { href: "/admin/oauth-clients", label: "OAuth clients", icon: KeyIcon },
  { href: "/admin/trusted-keys", label: "Trusted keys", icon: ShieldIcon },
];

function AdminNavList({ pathname }: { pathname: string }) {
  return (
    <nav aria-label="Administration">
      <ul className="space-y-0.5">
        {ADMIN_LINKS.map((l) => (
          <NavItem key={l.href} href={l.href} label={l.label} icon={l.icon} active={pathname.startsWith(l.href)} />
        ))}
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
  const base = orgSlug && wsSlug ? `/o/${orgSlug}/w/${wsSlug}` : null;
  return (
    <div className="flex h-full min-h-0 flex-col gap-4 p-4">
      <Link href="/" className="block self-start rounded-md" aria-label="Aurora home">
        <AuroraLogo className="h-[38px] w-auto" />
      </Link>
      {/* FIX-027: two stacked selects on every page — organization, then (only with an active
          organization) its workspaces. "All organizations" in the first one is the single path
          to the admin area; the account menu and the FIX-026 sidebar link are gone. */}
      <OrgSelector orgSlug={orgSlug} pathname={pathname} />
      {orgSlug ? <WorkspaceSelector orgSlug={orgSlug} wsSlug={wsSlug} pathname={pathname} /> : null}
      <div className="min-h-0 flex-1 overflow-y-auto">
        {base ? (
          <WorkspaceNav base={base} wsId={wsId} pathname={pathname} />
        ) : orgSlug ? (
          <OrgNav orgSlug={orgSlug} pathname={pathname} />
        ) : pathname.startsWith("/admin") ? (
          <AdminNavList pathname={pathname} />
        ) : pathname.startsWith("/account") ? (
          <AccountNavList pathname={pathname} />
        ) : null}
      </div>
      <div className="space-y-2 border-t border-line pt-3">
        <ThemeSwitch />
        <UserMenu />
      </div>
    </div>
  );
}

export function AppShell({ children }: { children: ReactNode }) {
  useUnauthorizedRedirect();
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
  const wsId = useResolve(orgSlug, wsSlug).data?.workspace?.id;
  const side = { orgSlug, wsSlug, wsId, pathname };

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
        </div>
        <main className="mx-auto w-full max-w-[1160px] space-y-4 px-4 py-6 md:px-8 md:py-8">
          <ApprovalNotifier />
          <FrozenEntityBanner />
          {children}
        </main>
        <Tour setNavOpen={tourNavOpen} />
      </div>
    </div>
  );
}
