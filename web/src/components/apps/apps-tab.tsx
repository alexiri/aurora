"use client";

import { useParams } from "next/navigation";
import { useCallback, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";

import { EmptyState, ErrorState, Loading } from "@/components/states";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent } from "@/components/ui/dialog";
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
      <section aria-label={`${label} tools`}>
        <h3 className="text-sm font-medium">{label} tools</h3>
        <ul className="list-disc pl-5 text-sm">
          {tools.map((t) => (
            <li key={t.id}>
              {t.name}
              {t.description ? <span className="text-neutral-600"> — {t.description}</span> : null}
            </li>
          ))}
        </ul>
      </section>
    ) : null;
  return (
    <div className="space-y-4 text-sm">
      <p>{d.description}</p>
      {!d.available ? <p role="status" className="text-amber-700">This app is not available to this workspace.</p> : null}
      {group("Read", reads)}
      {group("Write", writes)}
      <section aria-label="Hosts">
        <h3 className="text-sm font-medium">Hosts</h3>
        <ul className="space-y-1">
          {d.hosts.map((h) => (
            <li key={`${h.host}${h.path_prefix ?? ""}`}>
              <code>{h.host}{h.path_prefix ?? ""}</code>
              {h.claim_status === "lost" ? (
                <span className="ml-2 text-amber-700">
                  Another app currently claims this host, so credentials are not injected for this app.
                </span>
              ) : null}
            </li>
          ))}
        </ul>
      </section>
      {d.agent_instructions ? (
        <section aria-label="Agent instructions">
          <h3 className="text-sm font-medium">Instructions shown to agents</h3>
          <pre className="whitespace-pre-wrap rounded bg-neutral-50 p-2 text-xs">{d.agent_instructions}</pre>
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
    <li className="flex flex-col gap-2 rounded-md border border-neutral-200 p-3">
      <div className="flex items-center gap-2">
        {/* eslint-disable-next-line @next/next/no-img-element */}
        <img src={`/v1/apps/${encodeURIComponent(app.app_id)}/icon`} alt="" width={32} height={32} className="h-8 w-8 rounded" />
        <div>
          <p className="font-medium">{app.name}</p>
          <p className="text-xs text-neutral-500">
            {app.connection_count} connection{app.connection_count === 1 ? "" : "s"}
            {app.available ? "" : " · unavailable"}
          </p>
        </div>
      </div>
      <p className="line-clamp-2 text-sm text-neutral-600">{app.description}</p>
      <div className="mt-auto flex gap-2">
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
      <ul aria-label="Apps" className="grid gap-3 sm:grid-cols-2">
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
