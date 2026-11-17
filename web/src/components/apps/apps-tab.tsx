"use client";

import { useCallback, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";

import { EmptyState, ErrorState, Loading } from "@/components/states";
import { AppIcon } from "@/components/ui/app-icon";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent } from "@/components/ui/dialog";
import { Notice } from "@/components/ui/notice";
import { appIsCustomOnly, useApp, useApps, useConnectChannel, type AppSummary } from "@/lib/api/apps";
import { appDisplayName, productKey, productName } from "@/lib/api/app-name";

import { Markdown } from "@/components/ui/markdown";

import { ConnectJourney } from "./connect-journey/connect-journey";
import { useUrlDialog } from "./use-url-dialog";

export function AppDetailBody({ app, workspaceId }: { app: string; workspaceId: string }) {
  const detail = useApp(app, workspaceId);
  if (detail.isPending) return <Loading />;
  if (detail.isError) return <ErrorState error={detail.error} onRetry={() => detail.refetch()} />;
  const d = detail.data;
  const reads = d.tools.filter((t) => t.group === "read");
  const writes = d.tools.filter((t) => t.group === "write");
  const group = (label: string, tools: typeof d.tools) =>
    tools.length ? (
      <section aria-label={`${label} tools`} className="space-y-1.5">
        <h3 className="font-display text-sm font-semibold text-ink">{label} tools</h3>
        <ul className="list-disc space-y-0.5 pl-5 text-sm text-ink">
          {tools.map((t) => (
            <li key={t.id}>
              {t.name}
              {t.description ? <span className="text-ink-2"> — {t.description}</span> : null}
            </li>
          ))}
        </ul>
      </section>
    ) : null;
  return (
    <div className="space-y-4 text-sm text-ink">
      <p className="text-ink-2">{d.description}</p>
      {!d.available ? (
        <Notice tone="pending" role="status">
          This app is not available to this workspace.
        </Notice>
      ) : null}
      {group("Read", reads)}
      {group("Write", writes)}
      <section aria-label="Hosts" className="space-y-1.5">
        <h3 className="font-display text-sm font-semibold text-ink">Hosts</h3>
        <ul className="space-y-1.5">
          {d.hosts.map((h) => (
            <li key={`${h.host}${h.path_prefix ?? ""}`} className="space-y-1">
              <code className="break-all font-mono text-xs">{h.host}{h.path_prefix ?? ""}</code>
              {h.claim_status === "lost" ? (
                <Notice tone="pending" role="status" className="text-xs">
                  Another app currently claims this host, so credentials are not injected for this app.
                </Notice>
              ) : null}
            </li>
          ))}
        </ul>
      </section>
      {d.agent_instructions ? (
        <section aria-label="Agent instructions" className="space-y-1.5">
          <h3 className="font-display text-sm font-semibold text-ink">Instructions shown to agents</h3>
          <pre className="whitespace-pre-wrap rounded-control border border-line bg-subtle p-3 text-xs text-ink-2">{d.agent_instructions}</pre>
        </section>
      ) : null}
      {d.api_docs ? (
        <section aria-label="API documentation">
          <Markdown source={d.api_docs} />
        </section>
      ) : null}
    </div>
  );
}

/**
 * One tile per product (FIX-135, D-242): the apps of a family are one tile named by the family
 * (GitHub), its connect window asks where the product runs; an app without a family is its own tile.
 * `members[0]` is the hosted app of a family (the one without `instances`) when there is one.
 */
function AppCard({
  members,
  canOwn,
  onOpen,
  onConnect,
}: {
  members: AppSummary[];
  canOwn: boolean;
  onOpen: () => void;
  onConnect: () => void;
}) {
  const app = members[0]!;
  const name = productName(app);
  const connections = members.reduce((n, m) => n + m.connection_count, 0);
  const available = members.some((m) => m.available);
  return (
    <li className="flex flex-col gap-3 rounded-card border border-line bg-surface p-4 shadow-card sm:p-5">
      <div className="flex items-center gap-3">
        <AppIcon appId={app.app_id} name={name} hasDarkIcon={app.has_dark_icon} size={40} />
        <div className="min-w-0">
          <p className="truncate font-display text-base font-semibold text-ink">{name}</p>
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
        <Button size="sm" variant="outline" onClick={onOpen} aria-label={`Details of ${name}`}>Details</Button>
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
  const [open, setOpen] = useState<AppSummary | null>(null);
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
            onOpen={() => setOpen(members[0]!)}
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
      <Dialog open={!!open} onOpenChange={(o) => (o ? null : setOpen(null))}>
        {open ? (
          <DialogContent title={productName(open)} description={`${open.package_id} ${open.package_version}`}>
            <AppDetailBody app={open.app_id} workspaceId={workspaceId} />
          </DialogContent>
        ) : null}
      </Dialog>
    </>
  );
}
