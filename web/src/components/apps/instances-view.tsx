"use client";

import { useQueries } from "@tanstack/react-query";
import Link from "next/link";
import { useState, type FormEvent } from "react";

import { EmptyState, ErrorState, Loading } from "@/components/states";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { ConfirmDialog, FormDialog } from "@/components/ui/confirm-dialog";
import { DataTable, Td, Th } from "@/components/ui/data-table";
import { Field, inputClass } from "@/components/ui/field";
import { Notice } from "@/components/ui/notice";
import { Section } from "@/components/ui/section";
import {
  useAppInstances,
  useApps,
  useCreateAppInstance,
  useDeleteAppInstance,
  usePatchAppInstance,
  usePromoteAppInstance,
  type AppDetail,
  type AppInstance,
  type InstanceScope,
} from "@/lib/api/apps";
import { api } from "@/lib/api/client";
import { errorMessage, fieldErrors, hostConflict, hostConflictMessage } from "@/lib/api/errors";
import { useAdminOrgs } from "@/lib/api/freeze";
import { useMe } from "@/lib/api/queries";
import { unwrap } from "@/lib/api/unwrap";

/** Apps that declare `instances` (from the catalogue detail; no app knowledge lives in the bundle). */
export function useInstanceBoundApps() {
  const apps = useApps(undefined);
  const details = useQueries({
    queries: (apps.data?.items ?? []).map((a) => ({
      queryKey: ["app", a.app_id, ""],
      queryFn: () => unwrap(api.GET("/v1/apps/{app}", { params: { path: { app: a.app_id } } })),
      retry: false,
    })),
  });
  const bound = details
    .map((q) => q.data)
    .filter((d): d is AppDetail => !!d && d.instances != null);
  return { apps, bound, pending: apps.isPending || details.some((q) => q.isPending) };
}

/**
 * The refusal of a host (FIX-132): says who holds it as far as the server tells (an organization
 * administrator never sees another organization's name), links an instance administrator to the page
 * where it can be dealt with, and offers **Make instance-wide** for an organization's registration of the
 * app being registered.
 */
function HostConflictNotice({
  error,
  scope,
  appId,
  onPromoted,
}: {
  error: unknown;
  scope: InstanceScope;
  appId: string;
  onPromoted: () => void;
}) {
  const me = useMe();
  const promote = usePromoteAppInstance();
  const c = hostConflict(error);
  if (!c) {
    return (
      <Notice tone="danger" role="alert">
        {errorMessage(error)}
      </Notice>
    );
  }
  const isAdmin = me.data?.user.is_instance_admin === true;
  return (
    <Notice tone="danger" role="alert" className="space-y-2">
      <p>{hostConflictMessage(c, scope.kind === "organization")}</p>
      {c.kind === "static_claim" ? (
        isAdmin ? (
          <p>
            <Link href="/admin/host-claims" className="font-medium underline">
              Host claims
            </Link>{" "}
            ·{" "}
            <Link href="/admin/plugins" className="font-medium underline">
              Plugins
            </Link>
          </p>
        ) : (
          <p>Ask an instance administrator.</p>
        )
      ) : null}
      {c.kind === "instance" && c.scope === "org" && !c.promotable && isAdmin && scope.kind === "organization" ? (
        <p>
          <Link href="/admin/instances" className="font-medium underline">
            Self-hosted apps (admin)
          </Link>
        </p>
      ) : null}
      {c.promotable && c.instance_id && c.app_id === appId ? (
        <div className="space-y-1">
          <p>Make that registration available to every organization instead of registering the host again.</p>
          <Button
            size="sm"
            variant="outline"
            disabled={promote.isPending}
            onClick={() => promote.mutate(c.instance_id!, { onSuccess: onPromoted })}
          >
            Make instance-wide
          </Button>
          {promote.isError ? <p className="text-xs">{errorMessage(promote.error)}</p> : null}
        </div>
      ) : null}
    </Notice>
  );
}

function RegisterDialog({
  scope,
  open,
  onOpenChange,
  bound,
}: {
  scope: InstanceScope;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  bound: AppDetail[];
}) {
  const create = useCreateAppInstance(scope);
  const [appId, setAppId] = useState("");
  const [host, setHost] = useState("");
  const [label, setLabel] = useState("");
  const app = bound.find((a) => a.app_id === appId);
  const errs = fieldErrors(create.error);

  function reset() {
    setAppId("");
    setHost("");
    setLabel("");
    create.reset();
  }

  async function submit(e: FormEvent) {
    e.preventDefault();
    try {
      await create.mutateAsync({
        app_id: appId,
        host: host.trim(),
        label: label.trim() || host.trim(),
      });
      reset();
      onOpenChange(false);
    } catch {
      // shown in the dialog
    }
  }

  return (
    <FormDialog
      open={open}
      onOpenChange={(o) => {
        if (!o) reset();
        onOpenChange(o);
      }}
      title="Register a self-hosted app"
      description="Registering a host is a trust decision: it is the only place the app's credentials may be sent for connections bound to it."
      submitLabel="Register"
      busy={create.isPending}
      submitDisabled={!appId || !host.trim()}
      onSubmit={(e) => void submit(e)}
    >
      <Field id="ai-app" label="App" error={errs.app_id}>
        <select
          id="ai-app"
          className={inputClass}
          value={appId}
          onChange={(e) => setAppId(e.target.value)}
          required
        >
          <option value="">Select…</option>
          {bound.map((a) => (
            <option key={a.app_id} value={a.app_id}>
              {a.name}
            </option>
          ))}
        </select>
      </Field>
      <Field
        id="ai-host"
        label="Host"
        hint="The host name only, in lower case, for example gitlab.example.com. No https://, port, path or IP address."
        error={errs.host}
      >
        <input
          id="ai-host"
          className={`${inputClass} font-mono`}
          autoComplete="off"
          autoCapitalize="none"
          spellCheck={false}
          value={host}
          onChange={(e) => setHost(e.target.value)}
          required
        />
      </Field>
      <Field id="ai-label" label="Label" hint="Shown when people connect. Defaults to the host." error={errs.label}>
        <input
          id="ai-label"
          className={inputClass}
          placeholder={app?.instances?.label ?? ""}
          value={label}
          onChange={(e) => setLabel(e.target.value)}
        />
      </Field>
      {create.isError && !errs.app_id && !errs.label ? (
        <HostConflictNotice
          error={create.error}
          scope={scope}
          appId={appId}
          onPromoted={() => {
            reset();
            onOpenChange(false);
          }}
        />
      ) : null}
    </FormDialog>
  );
}

function InstanceRow({
  inst,
  appName,
  canManage,
  showOrg = false,
  canPromote = false,
}: {
  inst: AppInstance;
  appName: string;
  canManage: boolean;
  /** The admin list: an Organization column. */
  showOrg?: boolean;
  /** Offer **Make instance-wide** on an organization's registration (instance admin). */
  canPromote?: boolean;
}) {
  const patch = usePatchAppInstance();
  const del = useDeleteAppInstance();
  const promote = usePromoteAppInstance();
  const [confirm, setConfirm] = useState(false);
  const [confirmPromote, setConfirmPromote] = useState(false);
  const active = inst.status === "active";
  return (
    <tr>
      <Td>{appName}</Td>
      <Td>
        <div className="font-mono text-sm">{inst.host}</div>
        <div className="text-xs text-muted">{inst.label}</div>
      </Td>
      {showOrg ? <Td className="text-ink-2">{inst.org_id ? (inst.org_name ?? inst.org_slug ?? inst.org_id) : "—"}</Td> : null}
      <Td>
        <Badge tone={inst.org_id ? "neutral" : "accent"}>{inst.org_id ? "Organization" : "Instance-wide"}</Badge>
      </Td>
      <Td>
        <Badge tone={active ? "accent" : "pending"}>{active ? "Active" : "Disabled"}</Badge>
      </Td>
      <Td className="tabular-nums">{inst.connections}</Td>
      <Td className="text-right">
        {canPromote && inst.org_id ? (
          <>
            <Button
              size="sm"
              variant="outline"
              aria-label={`Make ${inst.host} instance-wide`}
              onClick={() => setConfirmPromote(true)}
            >
              Make instance-wide
            </Button>
            <ConfirmDialog
              open={confirmPromote}
              onOpenChange={setConfirmPromote}
              title={`Make ${inst.host} instance-wide?`}
              description={`Every organization will see this self-hosted app and can connect to it. Its ${inst.connections} existing connection${inst.connections === 1 ? "" : "s"} keep working. This is recorded in the audit log.`}
              confirmLabel="Make instance-wide"
              busy={promote.isPending}
              onConfirm={() => promote.mutate(inst.id, { onSuccess: () => setConfirmPromote(false) })}
            >
              {promote.isError ? (
                <Notice tone="danger" role="alert">
                  {errorMessage(promote.error)}
                </Notice>
              ) : null}
            </ConfirmDialog>
          </>
        ) : null}
        {canManage ? (
          <div className="flex flex-wrap justify-end gap-2">
            <Button
              size="sm"
              variant="outline"
              disabled={patch.isPending}
              aria-label={`${active ? "Disable" : "Enable"} ${inst.host}`}
              onClick={() => patch.mutate({ id: inst.id, status: active ? "disabled" : "active" })}
            >
              {active ? "Disable" : "Enable"}
            </Button>
            <Button size="sm" variant="danger" aria-label={`Delete ${inst.host}`} onClick={() => setConfirm(true)}>
              Delete
            </Button>
          </div>
        ) : null}
        {patch.isError ? (
          <p role="alert" className="mt-1 text-xs text-danger-ink">
            {errorMessage(patch.error)}
          </p>
        ) : null}
        <ConfirmDialog
          open={confirm}
          onOpenChange={setConfirm}
          title={`Delete ${inst.host}?`}
          description={`Its ${inst.connections} connection${inst.connections === 1 ? "" : "s"} will be disabled and its OAuth clients removed. Nothing is injected on this host again.`}
          confirmLabel="Delete self-hosted app"
          danger
          busy={del.isPending}
          onConfirm={() => del.mutate(inst.id, { onSuccess: () => setConfirm(false) })}
        >
          {del.isError ? (
            <Notice tone="danger" role="alert">
              {errorMessage(del.error)}
            </Notice>
          ) : null}
        </ConfirmDialog>
      </Td>
    </tr>
  );
}

/** The admin list's organization filter (instance admins only: it reads `/v1/admin/orgs`). */
function OrgFilter({ value, onChange }: { value: string; onChange: (v: string) => void }) {
  const orgs = useAdminOrgs(false);
  return (
    <select
      aria-label="Organization"
      className={`${inputClass} w-auto max-w-56`}
      value={value}
      onChange={(e) => onChange(e.target.value)}
    >
      <option value="">Every organization</option>
      {orgs.data?.items.map((o) => (
        <option key={o.id} value={o.id}>
          {o.name}
        </option>
      ))}
    </select>
  );
}

/**
 * Self-hosted apps: hosts registered for apps that can run on your own servers (D-74). Organization
 * owners/admins manage their organization's; instance admins see every registration (with its organization)
 * and manage the instance-wide ones, and can make an organization's registration instance-wide. Instance-wide
 * rows are read-only in an organization.
 */
export function InstancesView({ scope, canManage }: { scope: InstanceScope; canManage: boolean }) {
  const [orgFilter, setOrgFilter] = useState("");
  const adminAll = scope.kind === "instance" && scope.all === true;
  const effective: InstanceScope = adminAll ? { kind: "instance", all: true, org: orgFilter || undefined } : scope;
  const list = useAppInstances(effective);
  const { bound, pending } = useInstanceBoundApps();
  const [open, setOpen] = useState(false);
  if (list.isPending || pending) return <Loading />;
  if (list.isError) return <ErrorState error={list.error} onRetry={() => list.refetch()} />;
  const names = new Map(bound.map((a) => [a.app_id, a.name]));
  const items = list.data.items;
  return (
    <div className="space-y-6">
      <Notice tone="neutral" role="status">
        Register your own GitLab, GitHub Enterprise Server, Indico … so people can connect to it. An app that
        supports self-hosted servers is served on its SaaS host by default; register the host of each of your
        servers here and members then choose it when they connect. Credentials are only ever sent to the exact host
        a connection is bound to.
      </Notice>
      <Section
        id="self-hosted"
        title="Self-hosted apps"
        description={
          scope.kind === "instance"
            ? adminAll
              ? "Every registration: available to every organization, or registered by one organization."
              : "Available to every organization."
            : "Registered for this organization, plus the ones available to every organization."
        }
        actions={
          <div className="flex flex-wrap items-center gap-2">
            {adminAll ? <OrgFilter value={orgFilter} onChange={setOrgFilter} /> : null}
            {canManage && bound.length > 0 ? (
              <Button size="sm" onClick={() => setOpen(true)}>
                Register a self-hosted app
              </Button>
            ) : null}
          </div>
        }
      >
        {bound.length === 0 ? (
          <EmptyState title="No app supports self-hosting">
            Install or enable an app whose package declares self-hosted servers, such as the GitLab package.
          </EmptyState>
        ) : items.length === 0 ? (
          <EmptyState title="No self-hosted apps registered yet">
            Apps are connected to their SaaS host until you register one of your own servers.
          </EmptyState>
        ) : (
          <DataTable caption="Registered self-hosted apps">
            <thead>
              <tr>
                <Th>App</Th>
                <Th>Host</Th>
                {adminAll ? <Th>Organization</Th> : null}
                <Th>Scope</Th>
                <Th>Status</Th>
                <Th>Connections</Th>
                <Th>
                  <span className="sr-only">Actions</span>
                </Th>
              </tr>
            </thead>
            <tbody>
              {items.map((i) => (
                <InstanceRow
                  key={i.id}
                  inst={i}
                  appName={names.get(i.app_id) ?? i.app_id}
                  canManage={canManage && (adminAll ? i.org_id == null : scope.kind === "instance" || i.org_id != null)}
                  showOrg={adminAll}
                  canPromote={adminAll && canManage}
                />
              ))}
            </tbody>
          </DataTable>
        )}
      </Section>
      <RegisterDialog scope={scope} open={open} onOpenChange={setOpen} bound={bound} />
    </div>
  );
}
