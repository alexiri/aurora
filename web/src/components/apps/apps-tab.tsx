"use client";

import Link from "next/link";
import { useCallback } from "react";
import { useQueryClient } from "@tanstack/react-query";

import { EmptyState, ErrorState, Loading } from "@/components/states";
import { AppIcon } from "@/components/ui/app-icon";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Notice } from "@/components/ui/notice";
import { appIsCustomOnly, useApps, useConnectChannel, type AppSummary } from "@/lib/api/apps";
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
function AppCard({
  members,
  canOwn,
  onConnect,
}: {
  members: AppSummary[];
  canOwn: boolean;
  onConnect: () => void;
}) {
  const app = members[0]!;
  const name = productName(app);
  const connections = members.reduce((n, m) => n + m.connection_count, 0);
  const available = members.some((m) => m.available);
  const href = useAppHref()(app);
  return (
    <li className="flex flex-col gap-3 rounded-card border border-line bg-surface p-4 shadow-card sm:p-5">
      <div className="flex items-center gap-3">
        <AppIcon appId={app.app_id} name={name} hasDarkIcon={app.has_dark_icon} size={40} />
        <div className="min-w-0">
          <p className="truncate font-display text-base font-semibold text-ink">
            {href ? (
              <Link href={href} className="hover:underline">
                {name}
              </Link>
            ) : (
              name
            )}
          </p>
          <p className="text-xs text-muted">
            {connections} connection{connections === 1 ? "" : "s"}
            {available ? "" : " · unavailable"}
          </p>
        </div>
      </div>
      {appIsCustomOnly(app) ? (
        <div>
          <Badge title="Agents get only the tools you choose; full access is not offered for this app.">Custom access only</Badge>
        </div>
      ) : null}
      <p className="line-clamp-2 text-sm text-ink-2">{app.description}</p>
      <div className="mt-auto flex flex-wrap gap-2">
        {href ? (
          <Button asChild size="sm" variant="outline">
            <Link href={href} aria-label={`Details of ${name}`}>
              Details
            </Link>
          </Button>
        ) : null}
        {canOwn ? (
          <Button size="sm" disabled={!available} onClick={onConnect} aria-label={`Connect ${name}`}>Connect</Button>
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
  const products = [...grouped.values()].map((m) => [...m].sort((x, y) => Number(!!x.family?.instance_bound) - Number(!!y.family?.instance_bound)));
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
      <ul aria-label="Apps" className="grid gap-4 sm:grid-cols-2 xl:grid-cols-3">
        {products.map((members) => (
          <AppCard
            key={productKey(members[0]!)}
            members={members}
            canOwn={canOwn}
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
