"use client";

import Link from "next/link";
import { useCallback } from "react";
import { useQueryClient } from "@tanstack/react-query";

import { EmptyState, ErrorState, Loading } from "@/components/states";
import { AppIcon } from "@/components/ui/app-icon";
import { Button } from "@/components/ui/button";
import { Notice } from "@/components/ui/notice";
import { StatusLine } from "@/components/ui/status-line";
import { useAskSetup, useWithdrawSetup } from "@/lib/api/app-setup-requests";
import { useApps, useConnectChannel, type AppSummary } from "@/lib/api/apps";
import { appDisplayName, productKey, productName } from "@/lib/api/app-name";

import { useAppHref } from "./app-link";
import { ConnectJourney } from "./connect-journey/connect-journey";
import { useUrlDialog } from "./use-url-dialog";

/**
 * One tile per product (FIX-135, D-242): the apps of a family are one tile named by the family
 * (GitHub), its connect window asks where the product runs; an app without a family is its own tile.
 * `members[0]` is the hosted app of a family (the one without `instances`) when there is one.
 * (P5-UX-9, C-933) The name and **Details** lead to the app's setup page.
 */
/** What an app's card says about its state, and which button follows (P5-POLISH-7, D2; C-1029 readiness). */
export type AppStatus =
  | { kind: "connected"; line: string; ready: boolean }
  | { kind: "ready"; line: string }
  | { kind: "needs_admin"; line: string; admin: string | null; firstName: string | null; askedByMe: boolean };

const firstNameOf = (full: string | null | undefined) => (full ? full.trim().split(/\s+/)[0]! : null);

export function appStatus(members: AppSummary[]): AppStatus {
  const accounts = members.flatMap((m) => m.accounts ?? []);
  const connections = members.reduce((n, m) => n + m.connection_count, 0);
  const ready = members.some((m) => m.readiness?.state !== "needs_admin" && m.available !== false);
  if (connections > 0) {
    const line = accounts.length === 1 ? `Connected · ${accounts[0]!.name}` : `Connected · ${connections} accounts`;
    return { kind: "connected", line, ready };
  }
  if (!ready) {
    const r = members.find((m) => m.readiness?.state === "needs_admin")?.readiness;
    const admin = r?.who_can?.people[0]?.name ?? null;
    return { kind: "needs_admin", line: `Needs ${admin ?? "an admin"} first`, admin, firstName: firstNameOf(admin), askedByMe: r?.requested?.by_me === true };
  }
  return { kind: "ready", line: members.some((m) => m.credential_free) ? "Ready · no sign-in needed" : "Ready to connect" };
}

/** Switched off for the organization: needs setup and nothing to set up (C-1029). Such an app is not listed. */
export const isSwitchedOff = (members: AppSummary[]) =>
  members.every((m) => m.readiness?.state === "needs_admin" && (m.readiness.needs ?? []).length === 0);

function AppCard({
  members,
  canOwn,
  workspaceId,
  onConnect,
}: {
  members: AppSummary[];
  canOwn: boolean;
  workspaceId: string;
  onConnect: () => void;
}) {
  const app = members[0]!;
  const name = productName(app);
  const href = useAppHref()(app);
  const status = appStatus(members);
  const ask = useAskSetup(workspaceId);
  const withdraw = useWithdrawSetup(workspaceId);
  const text = app.summary ?? app.description;
  return (
    <li className="flex flex-col gap-3 rounded-card border border-line bg-surface p-4 shadow-card sm:p-5">
      <div className="flex items-center gap-3">
        <AppIcon appId={app.app_id} name={name} hasDarkIcon={app.has_dark_icon} size={36} />
        <p className="min-w-0 truncate font-display text-base font-semibold text-ink">
          {href ? (
            <Link href={href} className="hover:underline" aria-label={`Details of ${name}`}>
              {name}
            </Link>
          ) : (
            name
          )}
        </p>
      </div>
      {/* The summary whole, or the description clamped to two lines with its full text on hover. */}
      <p className={app.summary ? "text-sm text-ink-2" : "line-clamp-2 text-sm text-ink-2"} title={text}>
        {text}
      </p>
      <StatusLine tone={status.kind === "needs_admin" ? "idle" : status.kind === "connected" ? "ok" : "idle"} data-testid="app-status">
        {status.line}
      </StatusLine>
      <div className="mt-auto flex flex-wrap items-center gap-2">
        {canOwn && status.kind === "ready" ? (
          <Button size="sm" onClick={onConnect} aria-label={`Connect ${name}`}>
            Connect
          </Button>
        ) : null}
        {canOwn && status.kind === "connected" && status.ready ? (
          <Button size="sm" variant="outline" onClick={onConnect} aria-label={`Connect another ${name}`}>
            Connect another
          </Button>
        ) : null}
        {status.kind === "needs_admin" ? (
          status.askedByMe ? (
            <>
              <span className="text-sm text-ink-2">Asked {status.firstName ?? "an admin"}</span>
              <Button size="sm" variant="ghost" disabled={withdraw.isPending} onClick={() => withdraw.mutate(app.app_id)}>
                Undo
              </Button>
            </>
          ) : (
            <Button size="sm" variant="outline" disabled={ask.isPending} onClick={() => ask.mutate(app.app_id)} aria-label={`Ask ${status.firstName ?? "an admin"} to set up ${name}`}>
              Ask {status.firstName ?? "an admin"}
            </Button>
          )
        ) : null}
      </div>
    </li>
  );
}

export function AppsTab({ workspaceId, orgId, canOwn }: { workspaceId: string; orgId: string; canOwn: boolean }) {
  const apps = useApps(workspaceId);
  const qc = useQueryClient();
  // The Connect dialog (D-272): opened by a card's button or the deep link `?tab=apps&connect=<app>`.
  const [connecting, setConnecting] = useUrlDialog("connect");
  const refresh = useCallback(() => {
    void qc.invalidateQueries({ queryKey: ["apps"] });
    void qc.invalidateQueries({ queryKey: ["connections"] });
  }, [qc]);
  // Another dashboard tab finished a sign-in: refresh the lists (the waiting dialog handles its own result).
  useConnectChannel(refresh);

  if (apps.isPending) return <Loading />;
  if (apps.isError) return <ErrorState error={apps.error} onRetry={() => apps.refetch()} />;
  const items = apps.data.items;
  if (items.length === 0) return <EmptyState title="No apps installed" />;
  const target = connecting ? items.find((a) => a.app_id === connecting) : undefined;
  // One tile per product: a family's members together, the hosted one first (D-242).
  const grouped = new Map<string, AppSummary[]>();
  for (const a of items) grouped.set(productKey(a), [...(grouped.get(productKey(a)) ?? []), a]);
  const products = [...grouped.values()]
    .map((m) => [...m].sort((x, y) => Number(!!x.family?.instance_bound) - Number(!!y.family?.instance_bound)))
    // Apps switched off for the organization are not listed.
    .filter((m) => !isSwitchedOff(m))
    // (P5-POLISH-11) By display name, not by package id.
    .sort((a, b) => appDisplayName(a[0]!).localeCompare(appDisplayName(b[0]!)));
  return (
    <>
      {connecting && (!target || !target.available || !canOwn) ? (
        <Notice tone="pending" role="status" className="mb-4">
          {!target
            ? `The app “${connecting}” is not installed in this workspace.`
            : !canOwn
              ? `Only a workspace owner can connect ${appDisplayName(target)}.`
              : `${appDisplayName(target)} is not available to this workspace.`}
        </Notice>
      ) : null}
      <ul aria-label="Apps" className="grid gap-4 sm:grid-cols-2 lg:grid-cols-3">
        {products.map((members) => (
          <AppCard
            key={productKey(members[0]!)}
            members={members}
            canOwn={canOwn}
            workspaceId={workspaceId}
            onConnect={() => setConnecting((members.find((m) => m.available) ?? members[0]!).app_id)}
          />
        ))}
      </ul>
      {target && target.available && canOwn ? (
        <ConnectJourney
          appId={target.app_id}
          workspaceId={workspaceId}
          orgId={orgId}
          onClose={() => {
            setConnecting(null);
            refresh();
          }}
        />
      ) : null}
    </>
  );
}
