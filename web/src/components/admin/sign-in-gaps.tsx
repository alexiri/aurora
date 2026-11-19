"use client";

import Link from "next/link";
import { useQueries } from "@tanstack/react-query";

import { Notice } from "@/components/ui/notice";
import { buttonVariants } from "@/components/ui/button";
import { AppIcon } from "@/components/ui/app-icon";
import { Card } from "@/components/ui/card";
import { api } from "@/lib/api/client";
import { appKeys, useApps, type AppSetup } from "@/lib/api/apps";
import type { SetupCheck } from "@/lib/api/onboarding";
import { unwrap } from "@/lib/api/unwrap";
import { adminRoutes, orgRoutes } from "@/lib/routes";
import { cn } from "@/lib/utils";

/** An app people cannot sign in to for lack of an OAuth client (C-924). */
export type SignInGap = {
  appId: string;
  name: string;
  /** The package and OAuth client key the **Register an OAuth client** form is prefilled with. */
  pkg?: string;
  clientKey?: string;
  /** Another way to connect works (a token): informational only. */
  tokenOnly: boolean;
};

/** The setup status' `app_needs_oauth_client` / `app_token_only` checks (C-924), instance level. */
export function gapsFromChecks(checks: SetupCheck[]): SignInGap[] {
  return checks
    .filter((c) => (c.reason === "app_needs_oauth_client" || c.reason === "app_token_only") && c.app_id)
    .map((c) => ({
      appId: c.app_id!,
      name: c.value ?? c.app_id!,
      pkg: c.package ?? undefined,
      clientKey: c.client_key ?? undefined,
      tokenOnly: c.reason === "app_token_only",
    }));
}

/**
 * The same rule as the server's, for one organization's view of an app (C-931): a person's method
 * that needs only an OAuth client makes a gap; it is informational when another person's method works.
 */
export function gapFromSetup(s: AppSetup, pkg?: string): SignInGap | null {
  const people = s.methods.filter((m) => m.audience === "person");
  const only = (m: (typeof people)[number]) => m.needs.length === 1 && m.needs[0] === "oauth_client";
  const blocked = people.find((m) => only(m) && m.client_key);
  if (!blocked) return null;
  const works = people.some((m) => m.needs.length === 0);
  if (!works && !people.every((m) => m.needs.length === 0 || only(m))) return null;
  return { appId: s.app_id, name: s.name, pkg, clientKey: blocked.client_key ?? undefined, tokenOnly: works };
}

/** Apps of one package share one sign-in (Google Calendar, Drive, Gmail): one group, in the catalogue's order. */
export function groupGaps(gaps: SignInGap[]): SignInGap[][] {
  const groups = new Map<string, SignInGap[]>();
  for (const g of gaps) {
    const key = g.pkg ?? g.appId;
    groups.set(key, [...(groups.get(key) ?? []), g]);
  }
  return [...groups.values()];
}

/** "A", "A and B", "A, B and C". */
function joinNames(names: string[]): string {
  return names.length < 2 ? (names[0] ?? "") : `${names.slice(0, -1).join(", ")} and ${names[names.length - 1]}`;
}

type Where = { level: "admin"; orgSlug?: string } | { level: "org"; orgSlug: string };

/**
 * "Apps people can't sign in to yet" (C-924, P5-UX-6): one row per app whose ways to connect all need
 * an OAuth client that does not exist, with **Register an OAuth client** (prefilled) and a link to the
 * app's setup page; apps people can connect with a token get one informational line. Used on
 * Administration › Overview and on the organization's Apps tab.
 */
export function SignInGaps({ gaps, where }: { gaps: SignInGap[]; where: Where }) {
  const blocked = gaps.filter((g) => !g.tokenOnly);
  const tokenOnly = gaps.filter((g) => g.tokenOnly);
  const groups = groupGaps(blocked);
  if (gaps.length === 0) return null;
  const register = (g: SignInGap) => {
    const q = { ...(g.pkg ? { package: g.pkg } : {}), ...(g.clientKey ? { client_key: g.clientKey } : {}) };
    return where.level === "admin" ? adminRoutes.oauthClients(q) : orgRoutes(where.orgSlug).apps("oauth-clients", q);
  };
  const setupPage = (g: SignInGap) => (where.orgSlug ? orgRoutes(where.orgSlug).app(g.appId) : null);
  return (
    <section aria-labelledby="sign-in-gaps-h" data-testid="sign-in-gaps" className="space-y-3">
      <h2 id="sign-in-gaps-h" className="font-display text-base font-semibold text-ink">
        Apps people can&apos;t sign in to yet
      </h2>
      {blocked.length > 0 ? (
        <Card className="divide-y divide-line p-0">
          <ul aria-label="Apps people can't sign in to yet" className="divide-y divide-line">
            {groups.map((group) => {
              const g = group[0]!;
              const names = joinNames(group.map((x) => x.name));
              return (
                <li key={g.pkg ?? g.appId} data-app={g.appId} data-apps={group.map((x) => x.appId).join(" ")} className="flex flex-wrap items-center gap-x-4 gap-y-2 px-4 py-3 text-sm">
                  <AppIcon appId={g.appId} name={g.name} size={32} />
                  <div className="min-w-0 flex-1">
                    <p className="font-semibold text-ink">People can&apos;t sign in to {names} yet</p>
                    <p className="text-ink-2">
                      {group.length === 1 ? `${g.name} has no other way to connect.` : "They share one sign-in and have no other way to connect."} Register an OAuth client so
                      people can sign in.
                    </p>
                    {group.length === 1 && setupPage(g) ? (
                      <Link href={setupPage(g)!} className="text-sm font-medium text-ink underline underline-offset-2">
                        How to set up {g.name}
                      </Link>
                    ) : null}
                  </div>
                  <Link href={register(g)} className={cn(buttonVariants({ size: "sm" }), "text-on-primary!")}>
                    Register an OAuth client
                  </Link>
                </li>
              );
            })}
          </ul>
        </Card>
      ) : null}
      {tokenOnly.length > 0 ? (
        <Notice tone="neutral" role="status">
          People can connect {tokenOnly.map((g) => g.name).join(", ")} with a token. Signing in with{" "}
          {tokenOnly.length === 1 ? "it" : "them"} needs an OAuth client.{" "}
          {tokenOnly.map((g, i) => (
            <span key={g.appId}>
              {i > 0 ? " · " : ""}
              <Link href={register(g)} className="font-medium text-ink underline underline-offset-2">
                Register an OAuth client for {g.name}
              </Link>
            </span>
          ))}
        </Notice>
      ) : null}
    </section>
  );
}

/** The organization's Apps tab: the gaps of this organization, read from each app's setup status (C-931). Org admins only. */
export function OrgSignInGaps({ orgId, orgSlug }: { orgId: string; orgSlug: string }) {
  const apps = useApps(undefined);
  const items = apps.data?.items ?? [];
  const results = useQueries({
    queries: items.map((a) => ({
      queryKey: appKeys.setup(a.app_id, orgId),
      queryFn: () => unwrap(api.GET("/v1/apps/{app}/setup", { params: { path: { app: a.app_id }, query: { org_id: orgId } } })),
      retry: false,
    })),
  });
  const gaps = results.flatMap((r, i) => {
    const g = r.data ? gapFromSetup(r.data, items[i]?.package_id) : null;
    return g ? [g] : [];
  });
  return <SignInGaps gaps={gaps} where={{ level: "org", orgSlug }} />;
}
