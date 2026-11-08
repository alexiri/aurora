"use client";

import { useParams } from "next/navigation";
import { useCallback, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";

import { EmptyState, ErrorState, Loading } from "@/components/states";
import { AppIcon } from "@/components/ui/app-icon";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent } from "@/components/ui/dialog";
import { Notice } from "@/components/ui/notice";
import {
  connectWindowPath,
  openConnectWindow,
  useApp,
  useApps,
  useConnectedListener,
  type AppSummary,
} from "@/lib/api/apps";

import { Markdown } from "./markdown";

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

function AppCard({
  app,
  canOwn,
  onOpen,
  onConnect,
}: {
  app: AppSummary;
  canOwn: boolean;
  onOpen: () => void;
  onConnect: () => void;
}) {
  return (
    <li className="flex flex-col gap-3 rounded-card border border-line bg-surface p-4 shadow-card sm:p-5">
      <div className="flex items-center gap-3">
        <AppIcon appId={app.app_id} name={app.name} hasDarkIcon={app.has_dark_icon} size={40} />
        <div className="min-w-0">
          <p className="truncate font-display text-base font-semibold text-ink">{app.name}</p>
          <p className="text-xs text-muted">
            {app.connection_count} connection{app.connection_count === 1 ? "" : "s"}
            {app.available ? "" : " · unavailable"}
          </p>
        </div>
      </div>
      <p className="line-clamp-2 text-sm text-ink-2">{app.description}</p>
      <div className="mt-auto flex flex-wrap gap-2">
        <Button size="sm" variant="outline" onClick={onOpen} aria-label={`Details of ${app.name}`}>Details</Button>
        {canOwn ? (
          <Button size="sm" disabled={!app.available} onClick={onConnect} aria-label={`Connect ${app.name}`}>Connect</Button>
        ) : null}
      </div>
    </li>
  );
}

export function AppsTab({ workspaceId, canOwn }: { workspaceId: string; canOwn: boolean }) {
  const { org, ws } = useParams<{ org: string; ws: string }>();
  const apps = useApps(workspaceId);
  const qc = useQueryClient();
  const [open, setOpen] = useState<AppSummary | null>(null);
  const refresh = useCallback(() => {
    void qc.invalidateQueries({ queryKey: ["apps"] });
    void qc.invalidateQueries({ queryKey: ["connections"] });
  }, [qc]);
  useConnectedListener(refresh);

  if (apps.isPending) return <Loading />;
  if (apps.isError) return <ErrorState error={apps.error} onRetry={() => apps.refetch()} />;
  const items = apps.data.items;
  if (items.length === 0) return <EmptyState title="No apps installed" />;
  return (
    <>
      <ul aria-label="Apps" className="grid gap-4 sm:grid-cols-2 xl:grid-cols-3">
        {items.map((a) => (
          <AppCard
            key={a.app_id}
            app={a}
            canOwn={canOwn}
            onOpen={() => setOpen(a)}
            onConnect={() => openConnectWindow(connectWindowPath(org, ws, a.app_id))}
          />
        ))}
      </ul>
      <Dialog open={!!open} onOpenChange={(o) => (o ? null : setOpen(null))}>
        {open ? (
          <DialogContent title={open.name} description={`${open.package_id} ${open.package_version}`}>
            <AppDetailBody app={open.app_id} workspaceId={workspaceId} />
          </DialogContent>
        ) : null}
      </Dialog>
    </>
  );
}
