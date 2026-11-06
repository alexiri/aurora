"use client";

import Link from "next/link";
import { useParams, usePathname } from "next/navigation";
import type { ReactNode } from "react";

import { logout, useMe, useOrgs, useResolve, useWorkspaces } from "@/lib/api/queries";
import { loginUrl, useUnauthorizedRedirect } from "@/lib/auth/use-unauthorized-redirect";
import { cn } from "@/lib/utils";

import { Menu, MenuContent, MenuItem, MenuLabel, MenuTrigger } from "./ui/menu";

const SECTIONS = ["Conversations", "Memory", "Connections", "Keys", "Members", "Settings"];

function SwitcherButton({ label, value }: { label: string; value: string }) {
  return (
    <button
      type="button"
      aria-label={`${label}: ${value}`}
      className="flex items-center gap-1 rounded-md px-2 py-1 text-sm hover:bg-neutral-100"
    >
      <span className="text-neutral-500">{label}</span>
      <span className="font-medium">{value}</span>
      <span aria-hidden>▾</span>
    </button>
  );
}

function OrgSwitcher({ orgSlug }: { orgSlug?: string }) {
  const orgs = useOrgs();
  const current = orgs.data?.items.find((o) => o.slug === orgSlug);
  return (
    <Menu>
      <MenuTrigger asChild>
        <SwitcherButton label="Org" value={current?.name ?? orgSlug ?? "…"} />
      </MenuTrigger>
      <MenuContent>
        <MenuLabel>Organizations</MenuLabel>
        {orgs.isPending ? <MenuItem disabled>Loading…</MenuItem> : null}
        {orgs.isError ? <MenuItem disabled>Could not load organizations</MenuItem> : null}
        {orgs.data?.items.map((o) => (
          <MenuItem key={o.id} asChild>
            <Link href={`/o/${o.slug}`}>{o.name}</Link>
          </MenuItem>
        ))}
      </MenuContent>
    </Menu>
  );
}

function WorkspaceSwitcher({ orgSlug, wsSlug }: { orgSlug: string; wsSlug?: string }) {
  const resolved = useResolve(orgSlug);
  const workspaces = useWorkspaces(resolved.data?.org.id);
  const current = workspaces.data?.items.find((w) => w.slug === wsSlug);
  return (
    <Menu>
      <MenuTrigger asChild>
        <SwitcherButton label="Workspace" value={current?.name ?? wsSlug ?? "All"} />
      </MenuTrigger>
      <MenuContent>
        <MenuLabel>Workspaces</MenuLabel>
        {workspaces.isPending ? <MenuItem disabled>Loading…</MenuItem> : null}
        {workspaces.isError ? <MenuItem disabled>Could not load workspaces</MenuItem> : null}
        {workspaces.data?.items.length === 0 ? <MenuItem disabled>No workspaces</MenuItem> : null}
        {workspaces.data?.items.map((w) => (
          <MenuItem key={w.id} asChild>
            <Link href={`/o/${orgSlug}/w/${w.slug}`}>{w.name}</Link>
          </MenuItem>
        ))}
        <MenuItem asChild>
          <Link href={`/o/${orgSlug}`}>All workspaces…</Link>
        </MenuItem>
      </MenuContent>
    </Menu>
  );
}

function UserMenu() {
  const me = useMe();
  const name = me.data?.user.display_name;
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
          className="rounded-md px-2 py-1 text-sm hover:bg-neutral-100"
          aria-label={name ? `Account menu for ${name}` : "Account menu"}
        >
          {name ?? "Account"} <span aria-hidden>▾</span>
        </button>
      </MenuTrigger>
      <MenuContent align="end">
        {me.data ? <MenuLabel>{me.data.user.email}</MenuLabel> : null}
        <MenuItem onSelect={() => void onLogout()}>Sign out</MenuItem>
      </MenuContent>
    </Menu>
  );
}

export function AppShell({ children }: { children: ReactNode }) {
  useUnauthorizedRedirect();
  const params = useParams<{ org?: string; ws?: string }>();
  const pathname = usePathname();
  const orgSlug = params.org;
  const wsSlug = params.ws;
  const base = orgSlug && wsSlug ? `/o/${orgSlug}/w/${wsSlug}` : null;

  return (
    <div className="flex min-h-screen flex-col">
      <header className="flex items-center gap-2 border-b border-neutral-200 px-4 py-2">
        <Link href="/" className="mr-2 font-semibold">
          Aurora
        </Link>
        <nav aria-label="Context" className="flex items-center gap-1">
          <OrgSwitcher orgSlug={orgSlug} />
          {orgSlug ? <WorkspaceSwitcher orgSlug={orgSlug} wsSlug={wsSlug} /> : null}
        </nav>
        <div className="ml-auto">
          <UserMenu />
        </div>
      </header>
      <div className="flex flex-1">
        {base ? (
          <nav aria-label="Workspace" className="w-48 border-r border-neutral-200 p-3">
            <ul className="space-y-1 text-sm">
              <li>
                <Link
                  href={base}
                  aria-current={pathname === base ? "page" : undefined}
                  className={cn(
                    "block rounded px-2 py-1.5",
                    pathname === base ? "bg-neutral-100 font-medium" : "hover:bg-neutral-50",
                  )}
                >
                  Overview
                </Link>
              </li>
              {SECTIONS.map((s) => (
                <li key={s}>
                  <span
                    aria-disabled="true"
                    className="block cursor-not-allowed rounded px-2 py-1.5 text-neutral-400"
                  >
                    {s} <span className="text-xs">(coming soon)</span>
                  </span>
                </li>
              ))}
            </ul>
          </nav>
        ) : null}
        <main className="flex-1 p-6">{children}</main>
      </div>
    </div>
  );
}
