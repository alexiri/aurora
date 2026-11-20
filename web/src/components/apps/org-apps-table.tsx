"use client";

import Link from "next/link";
import { useQueries } from "@tanstack/react-query";
import { useState } from "react";

import { gapFromSetup, type SignInGap } from "@/components/admin/sign-in-gaps";
import { SetupDeclineButton } from "@/components/overview/setup-request-row";
import { ErrorState, Loading } from "@/components/states";
import { AppIcon } from "@/components/ui/app-icon";
import { Button, buttonVariants } from "@/components/ui/button";
import { Notice } from "@/components/ui/notice";
import { Segmented } from "@/components/ui/segmented";
import { StatusLine } from "@/components/ui/status-line";
import { Switch } from "@/components/ui/switch";
import { productKey, productName } from "@/lib/api/app-name";
import { useOrgSetupRequests, type AppSetupRequest } from "@/lib/api/app-setup-requests";
import { appKeys, toggleAllow, useAppAvailability, useApps, usePutAppAvailability, type AppAvailability, type AppSummary } from "@/lib/api/apps";
import { api } from "@/lib/api/client";
import { errorMessage } from "@/lib/api/errors";
import { unwrap } from "@/lib/api/unwrap";
import { orgRoutes } from "@/lib/routes";

type Filter = "all" | "setup" | "off";

/** One row: a product (all its variants), its status and whether people may connect it. */
export interface AppRow {
  key: string;
  name: string;
  /** The first member: its icon and id. */
  app: AppSummary;
  members: AppSummary[];
  /** "Cloud · Data Center", only for a product with variants. */
  hosts: string | null;
  /** The sign-in gap of the worst variant, when one needs a sign-in app. */
  gap: SignInGap | null;
}

const cap = (s: string) => `${s.charAt(0).toUpperCase()}${s.slice(1)}`;
const listWords = (names: string[]) => (names.length <= 2 ? names.join(" and ") : `${names.slice(0, -1).join(", ")} and ${names[names.length - 1]}`);

/** The provider a gap needs, from its OAuth client key ("google-calendar" and "google-drive" are "Google"). */
export function providerOf(g: SignInGap): string {
  const base = (g.clientKey ?? g.name).split(/[-_.]/)[0] ?? g.name;
  return cap(base);
}

/** Products (variants folded into one row), each with the worst gap of its variants. */
export function buildRows(apps: AppSummary[], gapById: Map<string, SignInGap>): AppRow[] {
  const groups = new Map<string, AppSummary[]>();
  for (const a of apps) groups.set(productKey(a), [...(groups.get(productKey(a)) ?? []), a]);
  const byName = (x: AppSummary[], y: AppSummary[]) => productName(x[0]!).localeCompare(productName(y[0]!), undefined, { sensitivity: "base" });
  return [...groups.values()].sort(byName).map((members) => {
    const sorted = [...members].sort((x, y) => Number(!!x.family?.instance_bound) - Number(!!y.family?.instance_bound));
    const first = sorted[0]!;
    const gap = sorted.map((m) => gapById.get(m.app_id)).find((g) => g && !g.tokenOnly) ?? null;
    const variants = sorted.map((m) => m.family?.variant_label).filter((v): v is string => !!v);
    return { key: productKey(first), name: productName(first), app: first, members: sorted, hosts: members.length > 1 && variants.length > 1 ? variants.join(" · ") : null, gap };
  });
}

/** The note's second sentence: providers that serve several apps ("One Google sign-in covers Calendar, Drive and Gmail."). */
export function providerSentence(rows: AppRow[]): string {
  const byProvider = new Map<string, string[]>();
  for (const r of rows) {
    if (!r.gap) continue;
    const p = providerOf(r.gap);
    byProvider.set(p, [...(byProvider.get(p) ?? []), r.name.replace(new RegExp(`^${p}\\s+`, "i"), "")]);
  }
  return [...byProvider.entries()]
    .filter(([, names]) => names.length > 1)
    .map(([p, names]) => `One ${p} sign-in covers ${listWords([...names].sort((a, b) => a.localeCompare(b)))}.`)
    .join(" ");
}

/** The availability after turning one app on or off, keeping what the page does not list (workspace entries, unknown apps). */
export function nextAvailability(cur: AppAvailability, allIds: string[], appId: string, on: boolean): AppAvailability {
  if (cur.posture !== "restricted") {
    if (on) return cur;
    // Turning one off while "every app" is on: only the other apps stay on.
    const orgWide = allIds.filter((id) => id !== appId).map((app_id) => ({ app_id }));
    const kept = cur.allow.filter((e) => e.workspace_id);
    return { posture: "restricted", allow: [...orgWide, ...kept] };
  }
  const isOn = cur.allow.some((e) => e.app_id === appId && !e.workspace_id);
  return isOn === on ? cur : { ...cur, allow: toggleAllow(cur.allow, appId, null) };
}

function askersText(r: AppSetupRequest): string {
  const names = r.askers.map((a) => a.display_name.split(/\s+/)[0]!).filter(Boolean);
  return `${listWords(names)} asked`;
}

/**
 * (P5-POLISH-9, E3) Organization › Apps & accounts › Apps as one table: App, Status ("Ready" or "Needs <Provider> sign-in" with
 * Set up) and **People may connect** (a switch that saves when you flip it, with Undo). A note counts the apps that need a
 * sign-in app and names the providers that cover several (Show them sets the filter); an open setup request shows on its
 * app's row ("Bob asked", Set up, Decline); the variants of one product are one row. It replaces "People can't sign in to X
 * yet" and the availability radio with its Save.
 */
export function OrgAppsTable({ orgId, orgSlug, canEdit }: { orgId: string; orgSlug: string; canEdit: boolean }) {
  const apps = useApps(undefined);
  const avail = useAppAvailability(orgId);
  const put = usePutAppAvailability(orgId);
  const requests = useOrgSetupRequests(orgId, canEdit);
  const items = apps.data?.items ?? [];
  const setups = useQueries({
    queries: items.map((a) => ({
      queryKey: appKeys.setup(a.app_id, orgId),
      queryFn: () => unwrap(api.GET("/v1/apps/{app}/setup", { params: { path: { app: a.app_id }, query: { org_id: orgId } } })),
      retry: false,
    })),
  });
  const [filter, setFilter] = useState<Filter>("all");
  const [last, setLast] = useState<{ name: string; on: boolean; before: AppAvailability } | null>(null);

  if (apps.isPending || avail.isPending) return <Loading />;
  if (apps.isError) return <ErrorState error={apps.error} onRetry={() => apps.refetch()} />;
  if (avail.isError) return <ErrorState error={avail.error} onRetry={() => avail.refetch()} />;

  const gapById = new Map<string, SignInGap>();
  setups.forEach((r, i) => {
    const g = r.data ? gapFromSetup(r.data, items[i]?.package_id) : null;
    if (g) gapById.set(items[i]!.app_id, g);
  });
  const allIds = items.map((a) => a.app_id);
  const current = avail.data;
  const isOn = (row: AppRow) => current.posture !== "restricted" || row.members.some((m) => current.allow.some((e) => e.app_id === m.app_id && !e.workspace_id));
  const rows = buildRows(items, gapById);
  const needsSetup = rows.filter((r) => r.gap);
  const off = rows.filter((r) => !isOn(r));
  const shown = filter === "setup" ? needsSetup : filter === "off" ? off : rows;
  const openRequests = (row: AppRow) => (requests.data ?? []).filter((r) => row.members.some((m) => m.app_id === r.app_id));
  const setUpHref = (g: SignInGap) => {
    const q = { ...(g.pkg ? { package: g.pkg } : {}), ...(g.clientKey ? { client_key: g.clientKey } : {}) };
    return orgRoutes(orgSlug).apps("oauth-clients", q);
  };

  function flip(row: AppRow, on: boolean) {
    const before = current;
    let next = current;
    for (const m of row.members) next = nextAvailability(next, allIds, m.app_id, on);
    setLast(null);
    put.mutate(next, { onSuccess: () => setLast({ name: row.name, on, before }) });
  }

  const sentence = providerSentence(rows);
  return (
    <div className="space-y-4">
      {needsSetup.length > 0 ? (
        <Notice tone="pending" role="status" className="flex flex-wrap items-center justify-between gap-3" data-testid="apps-setup-note">
          <span>
            {needsSetup.length === 1 ? "1 app needs" : `${needsSetup.length} apps need`} a sign-in app before people can connect {needsSetup.length === 1 ? "it" : "them"}.{" "}
            {sentence}
          </span>
          <Button size="sm" variant="outline" onClick={() => setFilter("setup")}>
            Show them
          </Button>
        </Notice>
      ) : null}
      <Segmented<Filter>
        label="Show apps"
        value={filter}
        onChange={setFilter}
        options={[
          { value: "all", label: `All ${rows.length}` },
          { value: "setup", label: `Needs setup ${needsSetup.length}` },
          { value: "off", label: `Off ${off.length}` },
        ]}
      />
      {put.error ? (
        <Notice tone="danger" role="alert">
          {errorMessage(put.error)}
        </Notice>
      ) : null}
      {last ? (
        <p role="status" className="flex flex-wrap items-center gap-2 text-sm text-ink-2" data-testid="apps-toast">
          {last.name} switched {last.on ? "on" : "off"}.
          <Button
            size="sm"
            variant="ghost"
            disabled={put.isPending}
            onClick={() => {
              put.mutate(last.before);
              setLast(null);
            }}
          >
            Undo
          </Button>
        </p>
      ) : null}
      <div className="overflow-x-auto rounded-card border border-line bg-surface shadow-card">
        <table className="w-full text-sm">
          <caption className="sr-only">Apps and whether people may connect them</caption>
          <thead>
            <tr className="border-b border-line text-left text-xs font-semibold text-muted">
              <th scope="col" className="px-4 py-2.5">App</th>
              <th scope="col" className="px-4 py-2.5">Status</th>
              <th scope="col" className="px-4 py-2.5">People may connect</th>
            </tr>
          </thead>
          <tbody className="divide-y divide-line">
            {shown.length === 0 ? (
              <tr>
                <td colSpan={3} className="px-4 py-4 text-ink-2">
                  No apps here.
                </td>
              </tr>
            ) : null}
            {shown.map((r) => {
              const reqs = openRequests(r);
              return (
                <tr key={r.key} data-app={r.app.app_id}>
                  <td className="px-4 py-3">
                    <div className="flex items-center gap-3">
                      <AppIcon appId={r.app.app_id} name={r.name} hasDarkIcon={r.app.has_dark_icon} size={32} />
                      <div className="min-w-0">
                        <p className="font-medium text-ink">{r.name}</p>
                        {r.hosts ? <p className="text-xs text-muted">{r.hosts}</p> : null}
                      </div>
                    </div>
                  </td>
                  <td className="px-4 py-3">
                    <div className="space-y-1">
                      {r.gap ? (
                        <StatusLine
                          tone="attention"
                          action={
                            canEdit ? (
                              <Link href={setUpHref(r.gap)} className={buttonVariants({ size: "sm", variant: "outline" })} aria-label={`Set up ${providerOf(r.gap)} sign-in for ${r.name}`}>
                                Set up
                              </Link>
                            ) : undefined
                          }>
                          Needs {providerOf(r.gap)} sign-in
                        </StatusLine>
                      ) : (
                        <StatusLine tone="ok">Ready</StatusLine>
                      )}
                      {reqs.map((q) => (
                        <p key={q.id} className="flex flex-wrap items-center gap-2 text-xs text-ink-2" data-testid="setup-request">
                          {askersText(q)}
                          {canEdit && r.gap ? (
                            <Link href={setUpHref(r.gap)} className="font-medium text-link underline">
                              Set up
                            </Link>
                          ) : null}
                          {canEdit ? (
                            <SetupDeclineButton request={q} orgId={orgId} noun={r.name} />
                          ) : null}
                        </p>
                      ))}
                    </div>
                  </td>
                  <td className="px-4 py-3">
                    <Switch
                      aria-label={`People may connect ${r.name}`}
                      checked={isOn(r)}
                      disabled={!canEdit || put.isPending}
                      onCheckedChange={(on) => flip(r, on)}
                    />
                  </td>
                </tr>
              );
            })}
          </tbody>
        </table>
      </div>
      {!canEdit ? <Notice role="status">Only owners and admins can change this.</Notice> : null}
    </div>
  );
}
