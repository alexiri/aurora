"use client";

import { useQueries } from "@tanstack/react-query";
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
  type AppDetail,
  type AppInstance,
  type InstanceScope,
} from "@/lib/api/apps";
import { api } from "@/lib/api/client";
import { errorMessage, fieldErrors } from "@/lib/api/errors";
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
      title="Register an instance"
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
      {create.isError && !errs.host && !errs.app_id && !errs.label ? (
        <Notice tone="danger" role="alert">
          {errorMessage(create.error)}
        </Notice>
      ) : null}
      {create.isError && errs.host ? (
        <Notice tone="danger" role="alert">
          {errorMessage(create.error)}
        </Notice>
      ) : null}
    </FormDialog>
  );
}

function InstanceRow({
  inst,
  appName,
  canManage,
}: {
  inst: AppInstance;
  appName: string;
  canManage: boolean;
}) {
  const patch = usePatchAppInstance();
  const del = useDeleteAppInstance();
  const [confirm, setConfirm] = useState(false);
  const active = inst.status === "active";
  return (
    <tr>
      <Td>{appName}</Td>
      <Td>
        <div className="font-mono text-sm">{inst.host}</div>
        <div className="text-xs text-muted">{inst.label}</div>
      </Td>
      <Td>
        <Badge tone={inst.org_id ? "neutral" : "accent"}>{inst.org_id ? "Organization" : "Instance-wide"}</Badge>
      </Td>
      <Td>
        <Badge tone={active ? "accent" : "pending"}>{active ? "Active" : "Disabled"}</Badge>
      </Td>
      <Td className="tabular-nums">{inst.connections}</Td>
      <Td className="text-right">
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
          confirmLabel="Delete instance"
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

/**
 * Self-hosted instances of instance-bound apps (D-74). Organization owners/admins manage their
 * organization's instances; instance admins manage the instance-wide ones. Instance-wide rows
 * are read-only in an organization.
 */
export function InstancesView({ scope, canManage }: { scope: InstanceScope; canManage: boolean }) {
  const list = useAppInstances(scope);
  const { bound, pending } = useInstanceBoundApps();
  const [open, setOpen] = useState(false);
  if (list.isPending || pending) return <Loading />;
  if (list.isError) return <ErrorState error={list.error} onRetry={() => list.refetch()} />;
  const names = new Map(bound.map((a) => [a.app_id, a.name]));
  const items = list.data.items;
  return (
    <div className="space-y-6">
      <Notice tone="neutral" role="status">
        An app that supports self-hosted deployments (for example your own GitLab) is served on its SaaS host by
        default. Register the host of each self-hosted deployment here; members then choose it when they connect.
        Credentials are only ever sent to the exact host a connection is bound to.
      </Notice>
      <Section
        id="instances"
        title="Instances"
        description={
          scope.kind === "instance"
            ? "Available to every organization."
            : "Registered for this organization, plus the instance-wide ones."
        }
        actions={
          canManage && bound.length > 0 ? (
            <Button size="sm" onClick={() => setOpen(true)}>
              Register instance
            </Button>
          ) : null
        }
      >
        {bound.length === 0 ? (
          <EmptyState title="No app supports self-hosted instances">
            Install or enable an app whose package declares instances, such as the GitLab package.
          </EmptyState>
        ) : items.length === 0 ? (
          <EmptyState title="No instances registered yet">
            Apps are connected to their SaaS host until you register a self-hosted instance.
          </EmptyState>
        ) : (
          <DataTable caption="Registered app instances">
            <thead>
              <tr>
                <Th>App</Th>
                <Th>Instance</Th>
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
                  canManage={canManage && (scope.kind === "instance" || i.org_id != null)}
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
