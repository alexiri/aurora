"use client";

import { useParams, useSearchParams } from "next/navigation";
import { useState, type FormEvent } from "react";

import { ErrorState, Loading } from "@/components/states";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { Field, inputClass } from "@/components/ui/field";
import { Form, FormActions } from "@/components/ui/form";
import { Notice } from "@/components/ui/notice";
import { PageHeader } from "@/components/ui/page-header";
import { errorMessage } from "@/lib/api/errors";
import {
  buildConnectFields,
  connectApp,
  CONNECT_MESSAGE,
  defaultValues,
  useApp,
  useAppGuide,
  useAppInstances,
  appIsCustomOnly,
  useOAuthClients,
  validateFields,
  type AppDetail,
  type ConnectMethod,
  type FieldValues,
} from "@/lib/api/apps";
import { useResolve } from "@/lib/api/queries";

import { GiveAccessPanel } from "@/components/credentials/give-access";

import { DynamicForm } from "./dynamic-form";
import { Markdown, safeHref } from "./markdown";

function ConnectForm({ app, workspaceId, orgId }: { app: AppDetail; workspaceId: string; orgId: string }) {
  const search = useSearchParams();
  const reconnect = search.get("reconnect") ?? undefined;
  // Instance-bound apps (D-74): the SaaS default host, or an instance an admin registered. A
  // reconnect keeps the connection's instance, so the picker is not shown then.
  const instanceBound = !!app.instances && !reconnect;
  const instances = useAppInstances(instanceBound ? { kind: "organization", id: orgId } : undefined);
  const [instanceId, setInstanceId] = useState("");
  const choices = (instances.data?.items ?? []).filter((i) => i.app_id === app.app_id && i.status === "active");
  const defaultHost = app.instances?.default_host ?? null;
  const [methodId, setMethodId] = useState(app.connect[0]?.id ?? "");
  const method: ConnectMethod | undefined = app.connect.find((m) => m.id === methodId);
  const [values, setValues] = useState<FieldValues>(() => defaultValues(app.connect[0]?.fields ?? []));
  const [scopes, setScopes] = useState<string[]>(app.connect[0]?.default_scopes ?? []);
  const [access, setAccess] = useState(app.connect[0]?.access_levels[0]?.id ?? "");
  const [clientId, setClientId] = useState("");
  const [errors, setErrors] = useState<Record<string, string>>({});
  const [submitError, setSubmitError] = useState<unknown>(null);
  const [busy, setBusy] = useState(false);
  const [done, setDone] = useState<{ missing: string[]; connectionId: string } | null>(null);
  const [accessDone, setAccessDone] = useState(false);
  const guide = useAppGuide(app.app_id);
  const clients = useOAuthClients({ kind: "workspace", id: workspaceId });
  // An OAuth client bound to an instance serves only that instance; the default host uses unbound ones.
  const matching = (clients.data?.items ?? []).filter(
    (c) =>
      c.enabled &&
      c.package_id === app.package_id &&
      c.client_key === method?.client_key &&
      (instanceBound ? (c.instance_id ?? "") === instanceId : !c.instance_id),
  );
  const needsInstanceClient = instanceBound && !!instanceId && method?.kind === "oauth2" && matching.length === 0;

  function pick(id: string) {
    const m = app.connect.find((x) => x.id === id);
    setMethodId(id);
    setValues(defaultValues(m?.fields ?? []));
    setScopes(m?.default_scopes ?? []);
    setAccess(m?.access_levels[0]?.id ?? "");
    setErrors({});
  }

  async function submit(e: FormEvent) {
    e.preventDefault();
    if (!method) return;
    const errs = validateFields(method.fields, values);
    if (instanceBound && !defaultHost && !instanceId) errs.__instance = "Choose an instance.";
    setErrors(errs);
    if (Object.keys(errs).length) return;
    setBusy(true);
    setSubmitError(null);
    try {
      const fields = buildConnectFields(method.fields, values);
      const res = await connectApp(app.app_id, {
        scope: "workspace",
        workspace_id: workspaceId,
        method_id: method.id,
        ...(Object.keys(fields).length ? { fields } : {}),
        ...(clientId ? { oauth_client_id: clientId } : {}),
        ...(instanceBound && instanceId ? { instance_id: instanceId } : {}),
        ...(access ? { access_level: access } : {}),
        ...(reconnect ? { reconnect_connection_id: reconnect } : {}),
      });
      if (res.kind === "redirect") {
        const url = safeHref(res.url);
        if (!url) throw new Error("The server returned an invalid redirect.");
        window.location.assign(url);
        return;
      }
      if (window.opener) {
        (window.opener as Window).postMessage({ type: CONNECT_MESSAGE, connection_id: res.connection.id, missing_scopes: res.missing_scopes }, window.location.origin);
      }
      // The window stays open for the "Give agents access" step (D-137); Skip or Done closes it.
      setDone({ missing: res.missing_scopes, connectionId: res.connection.id });
    } catch (err) {
      setSubmitError(err);
    } finally {
      setBusy(false);
      // Secrets never linger in state after a submit attempt.
      setValues((v) => {
        const next = { ...v };
        for (const f of method.fields) if (f.type === "secret") next[f.name] = "";
        return next;
      });
    }
  }

  if (done)
    return (
      <Card role="status" className="space-y-3 text-sm">
        <p className="font-medium text-ink">
          {app.name} is connected.{accessDone ? " You can close this window." : ""}
        </p>
        {done.missing.length ? (
          <Notice tone="pending">Some requested scopes were not granted: {done.missing.join(", ")}.</Notice>
        ) : null}
        {!accessDone ? (
          <GiveAccessPanel
            subject={{
              target: { kind: "connection", id: done.connectionId },
              scope: { kind: "workspace", id: workspaceId },
              orgId,
              appId: app.app_id,
              appWorkspaceId: workspaceId,
              customOnly: appIsCustomOnly(app),
            }}
            onDismiss={() => {
              setAccessDone(true);
              // A window opened by the Connections tab closes itself once the step is over.
              if (window.opener && done.missing.length === 0) window.close();
            }}
          />
        ) : null}
        {accessDone ? <Button size="sm" variant="outline" onClick={() => window.close()}>Close window</Button> : null}
      </Card>
    );

  return (
    <Form onSubmit={(e) => void submit(e)} className="max-w-xl rounded-card border border-line bg-surface p-4 shadow-card sm:p-5" aria-label={`Connect ${app.name}`} noValidate>
      {app.connect_note ? (
        <Notice>
          {app.connect_note.text}
          {app.connect_note.url && safeHref(app.connect_note.url) ? (
            <>
              {" "}
              <a className="break-all text-link underline" target="_blank" rel="noopener noreferrer" href={safeHref(app.connect_note.url)!}>
                {safeHref(app.connect_note.url)}
              </a>
            </>
          ) : null}
        </Notice>
      ) : null}
      {guide.data ? (
        <details className="rounded-control border border-line p-3">
          <summary className="min-h-9 cursor-pointer text-sm font-semibold text-ink">Setup guide</summary>
          <div className="mt-2"><Markdown source={guide.data} /></div>
        </details>
      ) : null}
      {instanceBound ? (
        <Field
          id="cm-instance"
          label={app.instances?.label ?? "Instance"}
          error={errors.__instance}
          hint={
            choices.length === 0 && !defaultHost
              ? "No instance is registered yet. Ask an organization admin to register one (Instances)."
              : "Credentials are only sent to the host you choose here."
          }
        >
          <select
            id="cm-instance"
            className={inputClass}
            value={instanceId}
            onChange={(e) => {
              setInstanceId(e.target.value);
              setClientId("");
            }}
          >
            {defaultHost ? <option value="">{`${defaultHost} (default)`}</option> : <option value="">Select…</option>}
            {choices.map((i) => (
              <option key={i.id} value={i.id}>{`${i.label} (${i.host})`}</option>
            ))}
          </select>
        </Field>
      ) : null}
      {needsInstanceClient ? (
        <Notice tone="pending" role="status">
          No OAuth application is registered for this instance yet. Ask an organization admin to add one under
          Credentials, OAuth clients, or connect with an access token instead.
        </Notice>
      ) : null}
      {app.connect.length > 1 ? (
        <Field id="cm-method" label="Connection method">
          <select id="cm-method" className={inputClass} value={methodId} onChange={(e) => pick(e.target.value)}>
            {app.connect.map((m) => (
              <option key={m.id} value={m.id}>{m.id}</option>
            ))}
          </select>
        </Field>
      ) : null}
      {method ? (
        <>
          <DynamicForm fields={method.fields} values={values} errors={errors} onChange={(n, v) => setValues((s) => ({ ...s, [n]: v }))} />
          {method.access_levels.length > 0 ? (
            <Field id="cm-access" label="Access level">
              <select id="cm-access" className={inputClass} value={access} onChange={(e) => setAccess(e.target.value)}>
                {method.access_levels.map((a) => (
                  <option key={a.id} value={a.id}>{a.label}</option>
                ))}
              </select>
            </Field>
          ) : null}
          {method.scopes.length > 0 ? (
            <fieldset className="min-w-0 space-y-2">
              <legend className="mb-1 text-sm font-medium text-ink">Requested scopes</legend>
              {method.scopes.map((s) => (
                <label key={s.scope} className="flex min-h-9 items-center gap-2.5 text-sm text-ink">
                  <input
                    type="checkbox"
                    className="size-4 shrink-0 accent-accent"
                    checked={scopes.includes(s.scope)}
                    onChange={(e) => setScopes((cur) => (e.target.checked ? [...cur, s.scope] : cur.filter((x) => x !== s.scope)))}
                  />
                  <span>{s.name}</span>
                  <Badge className="normal-case tracking-normal">{s.access === "write" ? "Read and write" : "Read only"}</Badge>
                </label>
              ))}
            </fieldset>
          ) : null}
          {method.kind === "oauth2" && method.client_key ? (
            <Field id="cm-client" label="OAuth client" hint="Leave on automatic to use the most specific enabled client.">
              <select id="cm-client" className={inputClass} value={clientId} onChange={(e) => setClientId(e.target.value)}>
                <option value="">Automatic</option>
                {matching.map((c) => (
                  <option key={c.id} value={c.id}>{c.scope} client ({c.fields.client_id ?? c.client_key})</option>
                ))}
              </select>
            </Field>
          ) : null}
        </>
      ) : null}
      {submitError ? <Notice tone="danger" role="alert">{errorMessage(submitError)}</Notice> : null}
      <FormActions>
        <Button type="submit" size="lg" disabled={busy || !method}>
          {busy ? "Connecting…" : method?.kind === "oauth2" ? "Continue to sign in" : reconnect ? "Reconnect" : "Connect"}
        </Button>
      </FormActions>
    </Form>
  );
}

export function ConnectView() {
  const { org, ws, app } = useParams<{ org: string; ws: string; app: string }>();
  const resolved = useResolve(org, ws);
  const wsId = resolved.data?.workspace?.id;
  const detail = useApp(app, wsId);
  if (resolved.isPending || (wsId && detail.isPending)) return <Loading />;
  if (resolved.isError) return <ErrorState error={resolved.error} />;
  if (!wsId) return <ErrorState error={{ code: "not_found" }} />;
  if (detail.isError) return <ErrorState error={detail.error} onRetry={() => detail.refetch()} />;
  if (!detail.data) return <Loading />;
  return (
    <div className="max-w-xl">
      <PageHeader
        title={`Connect ${detail.data.name}`}
        description="Give agents in this workspace access to this app through the gateway."
      />
      {detail.data.available ? (
        <ConnectForm app={detail.data} workspaceId={wsId} orgId={resolved.data!.org.id} />
      ) : (
        <ErrorState error={{ code: "app_unavailable" }} />
      )}
    </div>
  );
}
