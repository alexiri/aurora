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
// "impact" joins the mandatory review of a pending major upgrade (RFE-022, 09 §10).
// "conversions" joins it when the version converts agent grants (P4-GRANTS, D-123, 09 §10).
type Section = (typeof SECTIONS)[number] | "impact" | "conversions";

// Why a version waits for an instance admin although it may look like a routine upgrade (FIX-107).
const REASON_TEXT: Record<string, string> = {
  publisher_changed: "A different publisher ships this version of an installed package. Approve it only if you expect the change of publisher.",
  version_downgrade: "This version is older than one you already approved. Approving it rolls the package back.",
  builtin_id_reserved: "This package id belongs to a built-in package, and this version does not come from the built-in source.",
};

function reconnectLine(n: number): string {
  return `${n} ${n === 1 ? "connection" : "connections"} will need reconnecting`;
}

function convertLine(n: number): string {
  return `${n} agent ${n === 1 ? "grant" : "grants"} will be converted from full to custom access`;
}

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
  const impact = plugin.impact ?? null;
  const majorImpact = impact !== null && (impact.major_upgrade || impact.connections_needing_reauth > 0);
  const converting = impact !== null && (impact.grants_to_convert ?? 0) > 0;
  const required: Section[] = [...SECTIONS, ...(majorImpact ? (["impact"] as const) : []), ...(converting ? (["conversions"] as const) : [])];
  const allSeen = required.every((s) => seen.has(s));

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
      <Notice tone="accent" role="status" className="max-w-page-narrow">
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
    <div className="max-w-page-narrow space-y-5">
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
            {plugin.status_reason && REASON_TEXT[plugin.status_reason] ? (
              <p className="mt-1 text-ink-2">{REASON_TEXT[plugin.status_reason]}</p>
            ) : null}
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
          {plugin.apps.some((a) => a.library_host) ? (
            <>
              <dt className="text-muted">Skill library</dt>
              <dd className="text-ink">
                {plugin.apps
                  .filter((a) => a.library_host)
                  .map((a) => `${a.name}: can serve a skill library, reads repository files from ${a.library_host}`)
                  .join("; ")}
              </dd>
            </>
          ) : null}
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

      {majorImpact && impact.connections_needing_reauth > 0 ? (
        <Notice tone="danger" role="status" aria-label="Upgrade impact">
          <h2 className="font-medium">{reconnectLine(impact.connections_needing_reauth)}</h2>
          <p>
            {impact.major_upgrade ? "This is a major upgrade" : "This upgrade"}
            {impact.from_version ? ` from ${impact.from_version}` : ""} that changes the credential
            fields of {impact.connections_needing_reauth === 1 ? "a connection" : "these connections"}. Approving marks
            {impact.connections_needing_reauth === 1 ? " it" : " them"} as needing reconnection; agents using{" "}
            {impact.connections_needing_reauth === 1 ? "it" : "them"} are refused until a user reconnects.
          </p>
        </Notice>
      ) : null}

      {converting ? (
        <Notice tone="pending" role="status" aria-label="Grant conversion">
          <h2 className="font-medium">{convertLine(impact.grants_to_convert ?? 0)}</h2>
          <p>
            This version allows only custom access to its apps. Approving converts each full grant to a custom grant that
            allows exactly the tools the app has today, so agents keep what they can do with listed tools and lose only
            requests outside the tool list and tools added later. Nothing is converted back.
          </p>
        </Notice>
      ) : null}

      {converting ? (
        <div className="space-y-2">
          <Collapsible id="conversions" title="Grants to convert" open={opened.has("conversions")} onToggle={onToggle}>
            <p>{convertLine(impact.grants_to_convert ?? 0)}. The count is advisory; approval recomputes it.</p>
            <ul className="mt-2 list-disc pl-5">
              {impact.by_app
                .filter((a) => (a.grants_to_convert ?? 0) > 0)
                .map((a) => (
                  <li key={a.app_id}>
                    <strong>{a.app_id}</strong>: {a.grants_to_convert} {a.grants_to_convert === 1 ? "grant" : "grants"} in{" "}
                    {a.grant_workspaces ?? 0} {a.grant_workspaces === 1 ? "workspace" : "workspaces"}
                  </li>
                ))}
            </ul>
          </Collapsible>
        </div>
      ) : null}

      {majorImpact ? (
        <div className="space-y-2">
          <Collapsible id="impact" title="Impact on connections" open={opened.has("impact")} onToggle={onToggle}>
            <p>
              {reconnectLine(impact.connections_needing_reauth)} (of {impact.connections_total} live). The count is
              advisory; approval recomputes it.
            </p>
            {impact.by_app.length ? (
              <ul className="mt-2 list-disc pl-5">
                {impact.by_app.map((a) => (
                  <li key={a.app_id}>
                    <strong>{a.app_id}</strong>: {a.needing_reauth} of {a.connections} to reconnect
                  </li>
                ))}
              </ul>
            ) : null}
          </Collapsible>
        </div>
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
      {act.isSuccess && (act.data?.connections_reauth ?? 0) > 0 ? (
        <Notice tone="accent" role="status">
          Approved. {reconnectLine(act.data.connections_reauth)}: their owners see a Reconnect action on the connections page.
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
