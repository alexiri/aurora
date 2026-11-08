"use client";

import Link from "next/link";
import { useState, type ReactNode } from "react";

import { Button } from "@/components/ui/button";
import { Badge } from "@/components/ui/badge";
import { Card } from "@/components/ui/card";
import { Dialog, DialogContent } from "@/components/ui/dialog";
import { Field, inputClass } from "@/components/ui/field";
import { Notice } from "@/components/ui/notice";
import { PageHeader } from "@/components/ui/page-header";
import { ErrorState, Loading } from "@/components/states";
import { ApiError } from "@/lib/api/client";
import { errorMessage } from "@/lib/api/errors";
import {
  usePlugin,
  usePluginAction,
  useUninstallPlugin,
  type PluginPackage,
} from "@/lib/api/plugins";

import { ViolationList, statusTone } from "./plugins-view";

const SECTIONS = ["hooks", "egress", "context", "claims"] as const;
type Section = (typeof SECTIONS)[number];

function Collapsible({
  id,
  title,
  open,
  onToggle,
  children,
}: {
  id: Section;
  title: string;
  open: boolean;
  onToggle: (id: Section) => void;
  children: ReactNode;
}) {
  return (
    <section className="overflow-hidden rounded-card border border-line bg-surface shadow-card">
      <h3>
        <button
          type="button"
          aria-expanded={open}
          aria-controls={`sec-${id}`}
          className="min-h-11 w-full px-4 py-2 text-left text-sm font-medium text-ink hover:bg-subtle"
          onClick={() => onToggle(id)}
        >
          {open ? "▾" : "▸"} {title}
        </button>
      </h3>
      {open ? (
        <div id={`sec-${id}`} className="border-t border-line px-4 py-3 text-sm text-ink-2">
          {children}
        </div>
      ) : null}
    </section>
  );
}

const none = <p className="text-muted">None.</p>;

export function PluginReview({ plugin }: { plugin: PluginPackage }) {
  const act = usePluginAction(plugin.id);
  const uninstall = useUninstallPlugin(plugin.id);
  const [opened, setOpened] = useState<Set<Section>>(new Set());
  const [reason, setReason] = useState("");
  const [rejecting, setRejecting] = useState(false);
  const [liveCount, setLiveCount] = useState<number | null>(null);
  const [uninstalled, setUninstalled] = useState(false);

  const toggle = (s: Section) =>
    setOpened((prev) => {
      const n = new Set(prev);
      if (n.has(s)) n.delete(s);
      else n.add(s);
      return n;
    });
  // Gate on "has ever been expanded" so collapsing again does not re-lock.
  const [seen, setSeen] = useState<Set<Section>>(new Set());
  const onToggle = (s: Section) => {
    setSeen((p) => new Set(p).add(s));
    toggle(s);
  };
  const allSeen = SECTIONS.every((s) => seen.has(s));

  const doUninstall = (force: boolean) =>
    uninstall.mutate(force, {
      onSuccess: () => {
        setLiveCount(null);
        setUninstalled(true);
      },
      onError: (e) => {
        if (e instanceof ApiError && e.status === 409 && e.code === "live_connections") {
          const c = (e.details as { count?: number } | null)?.count;
          setLiveCount(typeof c === "number" ? c : 0);
        }
      },
    });

  if (uninstalled) {
    return (
      <Notice tone="accent" role="status" className="max-w-[880px]">
        Plugin uninstalled. <Link href="/admin/plugins" className="font-medium underline">Back to plugins</Link>
      </Notice>
    );
  }

  const pending = plugin.status === "pending_approval";
  const hooks = plugin.permissions.hooks ?? [];
  const context = plugin.permissions.context ?? [];
  const egress = Object.entries(plugin.permissions.egress ?? {});
  const actionError = act.isError ? act.error : null;
  const uninstallErr =
    uninstall.isError && liveCount === null ? uninstall.error : null;

  return (
    <div className="max-w-[880px] space-y-5">
      <PageHeader
        title={
          <>
            {plugin.package_id} <span className="font-mono text-xl font-normal text-muted">{plugin.version}</span>
          </>
        }
        breadcrumb={[{ label: "Plugins", href: "/admin/plugins" }, { label: plugin.package_id }]}
        className="mb-6"
      />
      <Card>
        <dl className="grid grid-cols-[max-content_1fr] gap-x-6 gap-y-2 text-sm">
          <dt className="text-muted">Status</dt>
          <dd>
            <Badge tone={statusTone(plugin.status)}>{plugin.status.replace("_", " ")}</Badge>
            {plugin.status_reason ? <span className="ml-2 text-ink-2">({plugin.status_reason})</span> : null}
          </dd>
          <dt className="text-muted">Publisher</dt>
          <dd className="text-ink">{plugin.publisher}</dd>
          <dt className="text-muted">Signer</dt>
          <dd className="text-ink">
            {plugin.signer.kind === "trusted_key"
              ? `Trusted key ${plugin.signer.name ?? ""} ${plugin.signer.key_id ?? ""}`
              : plugin.signer.kind}
            {plugin.signer.kind === "unsigned" ? (
              <span className="ml-2 text-pending-ink">Unsigned package: development use only.</span>
            ) : null}
          </dd>
          <dt className="text-muted">Digest</dt>
          <dd className="break-all font-mono text-xs text-ink-2">{plugin.digest}</dd>
          <dt className="text-muted">Apps</dt>
          <dd className="text-ink">{plugin.apps.map((a) => a.name).join(", ") || "None"}</dd>
        </dl>
      </Card>

      {plugin.violations.length > 0 ? <ViolationList violations={plugin.violations} /> : null}

      {plugin.widenings.length > 0 ? (
        <Notice tone="pending">
          <h2 className="font-medium">Changes versus the previously approved version</h2>
          <ul className="list-disc pl-5">
            {plugin.widenings.map((w, i) => (
              <li key={i}>
                <strong>{w.kind}</strong>: {w.detail}
              </li>
            ))}
          </ul>
        </Notice>
      ) : null}

      <div className="space-y-2">
        <h2 className="font-display text-base font-semibold text-ink">Permissions</h2>
        <Collapsible id="hooks" title="Hooks" open={opened.has("hooks")} onToggle={onToggle}>
          {hooks.length ? (
            <ul className="list-disc pl-5">
              {hooks.map((h) => (
                <li key={h}>{h}</li>
              ))}
            </ul>
          ) : (
            none
          )}
        </Collapsible>
        <Collapsible id="egress" title="Egress per hook" open={opened.has("egress")} onToggle={onToggle}>
          {egress.length ? (
            <ul className="space-y-1">
              {egress.map(([hook, hosts]) => (
                <li key={hook}>
                  <strong>{hook}</strong>: {hosts.join(", ") || "no hosts"}
                </li>
              ))}
            </ul>
          ) : (
            none
          )}
        </Collapsible>
        <Collapsible id="context" title="Context variables" open={opened.has("context")} onToggle={onToggle}>
          {context.length ? (
            <ul className="list-disc pl-5">
              {context.map((c) => (
                <li key={c}>{c}</li>
              ))}
            </ul>
          ) : (
            none
          )}
        </Collapsible>
        <Collapsible id="claims" title="Host claims" open={opened.has("claims")} onToggle={onToggle}>
          {plugin.host_claims.length ? (
            <ul className="list-disc pl-5">
              {plugin.host_claims.map((c) => (
                <li key={c.claim_key}>
                  {c.host}
                  {c.path_prefix ?? ""} ({c.app_id})
                </li>
              ))}
            </ul>
          ) : (
            none
          )}
        </Collapsible>
      </div>

      <div className="flex flex-wrap gap-2">
        {pending ? (
          <>
            <Button
              variant="approve"
              disabled={!allSeen || act.isPending}
              onClick={() => act.mutate({ action: "approve" })}
            >
              Approve
            </Button>
            {!allSeen ? (
              <span className="self-center text-xs text-muted">
                Expand every permissions section to enable approval.
              </span>
            ) : null}
            <Button variant="danger" disabled={act.isPending} onClick={() => setRejecting(true)}>
              Reject
            </Button>
          </>
        ) : null}
        {plugin.status === "active" ? (
          <Button variant="outline" disabled={act.isPending} onClick={() => act.mutate({ action: "disable" })}>
            Disable
          </Button>
        ) : null}
        {plugin.status === "disabled" ? (
          <Button variant="outline" disabled={act.isPending} onClick={() => act.mutate({ action: "enable" })}>
            Enable
          </Button>
        ) : null}
        <Button variant="danger" disabled={uninstall.isPending} onClick={() => doUninstall(false)}>
          Uninstall
        </Button>
      </div>
      {actionError ? (
        <Notice tone="danger" role="alert">
          {actionError instanceof ApiError && actionError.code === "claim_conflict_unresolved"
            ? "Resolve the host claim conflicts first."
            : errorMessage(actionError)}
          {actionError instanceof ApiError && actionError.code === "claim_conflict_unresolved" ? (
            <>
              {" "}
              <Link href="/admin/host-claims" className="font-medium underline">
                Host claims
              </Link>
            </>
          ) : null}
        </Notice>
      ) : null}
      {uninstallErr ? (
        <Notice tone="danger" role="alert">
          {errorMessage(uninstallErr)}
        </Notice>
      ) : null}

      <Dialog open={rejecting} onOpenChange={setRejecting}>
        <DialogContent title="Reject plugin" description="Optionally tell others why.">
          <form
            className="space-y-3"
            onSubmit={(e) => {
              e.preventDefault();
              act.mutate({ action: "reject", reason: reason.trim() }, { onSuccess: () => setRejecting(false) });
            }}
          >
            <Field id="reject-reason" label="Reason">
              <input
                id="reject-reason"
                className={inputClass}
                value={reason}
                onChange={(e) => setReason(e.target.value)}
              />
            </Field>
            <div className="flex justify-end">
              <Button type="submit" variant="danger" disabled={act.isPending}>
                Confirm reject
              </Button>
            </div>
          </form>
        </DialogContent>
      </Dialog>

      <Dialog open={liveCount !== null} onOpenChange={(o) => !o && setLiveCount(null)}>
        <DialogContent
          title="Plugin has live connections"
          description={`${liveCount ?? 0} connection(s) use this plugin. Uninstalling will mark them orphaned.`}
        >
          <div className="flex flex-wrap justify-end gap-2">
            <Button variant="outline" onClick={() => setLiveCount(null)}>
              Cancel
            </Button>
            <Button variant="danger" disabled={uninstall.isPending} onClick={() => doUninstall(true)}>
              Uninstall anyway
            </Button>
          </div>
        </DialogContent>
      </Dialog>
    </div>
  );
}

export function PluginDetailView({ id }: { id: string }) {
  const q = usePlugin(id);
  if (q.isPending) return <Loading />;
  if (q.isError) return <ErrorState error={q.error} onRetry={() => q.refetch()} />;
  return <PluginReview key={`${q.data.id}`} plugin={q.data} />;
}
