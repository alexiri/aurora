"use client";

import { useQueryClient } from "@tanstack/react-query";
import Link from "next/link";
import { useCallback, useEffect, useState, type FormEvent } from "react";

import { ErrorState, Loading } from "@/components/states";
import { Button } from "@/components/ui/button";
import { ChoiceCards } from "@/components/ui/choice-cards";
import { Dialog, DialogContent } from "@/components/ui/dialog";
import { Field, inputClass } from "@/components/ui/field";
import { Form, FormActions } from "@/components/ui/form";
import { Notice } from "@/components/ui/notice";
import { appDisplayName } from "@/lib/api/app-name";
import { errorMessage } from "@/lib/api/errors";
import { useMe } from "@/lib/api/queries";
import { adminRoutes } from "@/lib/routes";
import {
  buildConnectFields,
  connectApp,
  defaultValues,
  useApp,
  useAppGuide,
  useAppInstances,
  appIsCustomOnly,
  useConnections,
  useOAuthClients,
  validateFields,
  type AppDetail,
  type AppSummary,
  type ConnectMethod,
  type FieldValues,
} from "@/lib/api/apps";

import { GiveAccessPanel } from "@/components/credentials/give-access";
import { OAuthClientField, RequestedScopes, TokenScopes } from "./connect-journey/advanced-fields";
import { useSignInWindow } from "./connect-journey/use-sign-in-window";
import { DynamicForm } from "./dynamic-form";
import { Markdown, safeHref } from "@/components/ui/markdown";

/**
 * (FIX-135, D-242) The first question for an app of a family: where does it run? The hosted
 * deployment (github.com), then every registered server of the family's instance-bound apps, then
 * how to register one. Each choice is one app (and instance); the gateway guarantee is unchanged.
 */
export function DeploymentStep({
  family,
  siblings,
  orgId,
  onPick,
}: {
  family: NonNullable<AppDetail["family"]>;
  siblings: AppSummary[];
  orgId: string;
  onPick: (w: { appId: string; instanceId?: string }) => void;
}) {
  const instances = useAppInstances({ kind: "organization", id: orgId });
  const me = useMe();
  const hosted = siblings.filter((a) => !a.family?.instance_bound);
  const bound = siblings.filter((a) => a.family?.instance_bound);
  const servers = (instances.data?.items ?? []).filter((i) => i.status === "active" && bound.some((a) => a.app_id === i.app_id));
  const options = [
    ...hosted.map((a) => ({ value: `app:${a.app_id}`, label: a.family!.variant_label, description: undefined as string | undefined })),
    ...servers.map((i) => {
      const a = bound.find((x) => x.app_id === i.app_id)!;
      return { value: `inst:${i.app_id}:${i.id}`, label: `${a.family!.variant_label} · ${i.label}`, description: i.base_url };
    }),
  ];
  const [value, setValue] = useState(options[0]?.value ?? "");
  const chosen = options.find((o) => o.value === value) ?? options[0];
  function go() {
    if (!chosen) return;
    const [kind, app, inst] = chosen.value.split(":");
    onPick(kind === "inst" ? { appId: app!, instanceId: inst } : { appId: chosen.value.slice(4) });
  }
  return (
    <div className="space-y-4">
      <ChoiceCards
        legend={`Where does your ${family.name} run?`}
        value={chosen?.value ?? ""}
        onChange={setValue}
        options={options}
      />
      {bound.length > 0 && servers.length === 0 ? (
        <Notice tone="pending" role="status">
          No self-hosted server is registered yet.{" "}
          {me.data?.user.is_instance_admin ? (
            <Link href={adminRoutes.selfHosted()} className="font-medium underline">
              Register a server… (Administration › Self-hosted apps)
            </Link>
          ) : (
            "Ask an administrator to register one (Self-hosted apps)."
          )}
        </Notice>
      ) : bound.length > 0 && me.data?.user.is_instance_admin ? (
        <p className="text-xs text-muted">
          Another server?{" "}
          <Link href={adminRoutes.selfHosted()} className="font-medium underline">
            Register a server… (Administration › Self-hosted apps)
          </Link>
        </p>
      ) : null}
      <FormActions>
        <Button type="button" disabled={!chosen} onClick={go}>
          Continue
        </Button>
      </FormActions>
    </div>
  );
}

/** The method the chooser preselects: the one a reconnect keeps, else the recommended one, else the first. */
function initialMethod(app: AppDetail, keep?: string | null): ConnectMethod | undefined {
  return (
    (keep ? app.connect.find((m) => m.id === keep) : undefined) ?? app.connect.find((m) => m.recommended) ?? app.connect[0]
  );
}

const KIND_LABEL: Record<string, string> = { oauth2: "Sign in", fields: "Access token or keys", hook: "Custom" };

function ConnectForm({
  app,
  workspaceId,
  orgId,
  keepMethod,
  reconnect,
  orgScope,
  onClose,
}: {
  app: AppDetail;
  workspaceId: string;
  orgId: string;
  /** (P5-UX-11, C-842, C-937) Reconnecting an organization's account: the request is organization-scoped. */
  orgScope?: boolean;
  /** A reconnect keeps the connection's method unless the person picks another (D-214). */
  keepMethod?: string | null;
  /** The id of the connection being reconnected; it keeps its instance, so no instance is asked for. */
  reconnect: string;
  onClose: () => void;
}) {
  const qc = useQueryClient();
  const first = initialMethod(app, keepMethod);
  const [methodId, setMethodId] = useState(first?.id ?? "");
  const method: ConnectMethod | undefined = app.connect.find((m) => m.id === methodId);
  const [values, setValues] = useState<FieldValues>(() => defaultValues(first?.fields ?? []));
  const [scopes, setScopes] = useState<string[]>(first?.default_scopes ?? []);
  const [access, setAccess] = useState(first?.access_levels[0]?.id ?? "");
  // (FIX-136) A pasted token's scopes: detected by the method's read probes, or ticked by the person.
  const [scopeMode, setScopeMode] = useState<"detect" | "choose">("detect");
  const [clientId, setClientId] = useState("");
  const [errors, setErrors] = useState<Record<string, string>>({});
  const [submitError, setSubmitError] = useState<unknown>(null);
  const [busy, setBusy] = useState(false);
  // `scopes` is unknown when the result came over the channel after an OAuth sign-in; the step looks it up.
  const [done, setDone] = useState<{ missing: string[]; connectionId: string; scopes?: string[] } | null>(null);
  const [accessDone, setAccessDone] = useState(false);
  const refreshLists = useCallback(() => {
    void qc.invalidateQueries({ queryKey: ["apps"] });
    void qc.invalidateQueries({ queryKey: ["connections"] });
  }, [qc]);
  // (C-842) OAuth: the provider's sign-in runs in a separate window while the dialog waits for its result.
  const signIn = useSignInWindow(
    app.app_id,
    useCallback(
      (m) => {
        refreshLists();
        setDone({ missing: m.missingScopes, connectionId: m.connectionId });
      },
      [refreshLists],
    ),
  );
  const { phase, blocked, notice: flowNotice } = signIn;
  const guide = useAppGuide(app.app_id);
  const clients = useOAuthClients(orgScope ? { kind: "organization", id: orgId } : { kind: "workspace", id: workspaceId });
  const matching = (clients.data?.items ?? []).filter(
    (c) => c.enabled && c.package_id === app.package_id && c.client_key === method?.client_key && !c.instance_id,
  );

  function pick(id: string) {
    const m = app.connect.find((x) => x.id === id);
    setMethodId(id);
    setValues(defaultValues(m?.fields ?? []));
    setScopes(m?.default_scopes ?? []);
    setAccess(m?.access_levels[0]?.id ?? "");
    setErrors({});
  }

  const closeSignIn = signIn.close;
  const reopenSignIn = signIn.reopen;
  const cancelSignIn = signIn.cancel;

  function submit(e: FormEvent) {
    e.preventDefault();
    if (!method) return;
    const errs = validateFields(method.fields, values);
    setErrors(errs);
    if (Object.keys(errs).length) return;
    // The sign-in window is opened here, synchronously in the click, so popup blockers allow it (C-842).
    let w: Window | null = null;
    if (method.kind === "oauth2") {
      w = signIn.open();
      if (!w) return;
    } else {
      signIn.setNotice(null);
      signIn.setBlocked(false);
    }
    void send(method, w);
  }

  async function send(method: ConnectMethod, w: Window | null) {
    setBusy(true);
    setSubmitError(null);
    try {
      const fields = buildConnectFields(method.fields, values);
      const res = await connectApp(app.app_id, {
        ...(orgScope ? { scope: "organization" as const, org_id: orgId } : { scope: "workspace" as const, workspace_id: workspaceId }),
        method_id: method.id,
        ...(Object.keys(fields).length ? { fields } : {}),
        ...(clientId ? { oauth_client_id: clientId } : {}),
        ...(access ? { access_level: access } : {}),
        ...(method.kind === "fields" && method.scopes.length > 0 && (scopeMode === "choose" || method.probe_scopes.length === 0)
          ? { scopes }
          : {}),
        reconnect_connection_id: reconnect,
      });
      if (res.kind === "redirect") {
        const url = safeHref(res.url);
        if (!url || !w) throw new Error("The server returned an invalid redirect.");
        signIn.navigate(w, url);
        return;
      }
      closeSignIn();
      refreshLists();
      // The dialog stays open for the "Give agents access" step (D-137).
      setDone({ missing: res.missing_scopes, connectionId: res.connection.id, scopes: res.connection.scopes ?? [] });
    } catch (err) {
      closeSignIn();
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
      <div role="status" className="space-y-3 text-sm">
        <p className="font-medium text-ink">{app.name} is connected.</p>
        {done.missing.length ? (
          <Notice tone="pending">Some of the access you asked for was not given: {done.missing.join(", ")}.</Notice>
        ) : null}
        {!accessDone && !orgScope ? (
          <ConnectedStep
            app={app}
            workspaceId={workspaceId}
            orgId={orgId}
            methodId={method?.id}
            done={done}
            // Nothing left to read once the step is over, unless scopes were refused: then the notice stays.
            onDismiss={() => (done.missing.length === 0 ? onClose() : setAccessDone(true))}
          />
        ) : (
          <Button size="sm" variant="outline" onClick={onClose}>Done</Button>
        )}
      </div>
    );

  if (phase === "waiting")
    return (
      <div role="status" className="space-y-3 text-sm">
        <p className="font-medium text-ink">Waiting for you to sign in to {app.name}…</p>
        <p className="text-ink-2">Finish in the sign-in window. This dialog continues when you are done.</p>
        {blocked ? (
          <Notice tone="pending" role="alert">
            Your browser blocked the sign-in window. Allow pop-ups for this site, then open it again.
          </Notice>
        ) : null}
        <div className="flex flex-wrap gap-2">
          <Button size="sm" variant="outline" onClick={reopenSignIn}>Open the sign-in window again</Button>
          <Button size="sm" variant="ghost" onClick={cancelSignIn}>Cancel</Button>
        </div>
      </div>
    );

  return (
    <Form onSubmit={submit} aria-label={`Connect ${app.name}`} noValidate>
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
      {app.connect.length > 1 ? (
        <ChoiceCards
          legend="How do you want to connect?"
          value={methodId}
          onChange={pick}
          options={app.connect.map((m) => ({
            value: m.id,
            label: m.label ?? KIND_LABEL[m.kind] ?? m.id,
            description: m.description ?? undefined,
            badge: m.recommended ? "Recommended" : undefined,
          }))}
        />
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
          {method.scopes.length > 0 && method.kind !== "fields" ? (
            <RequestedScopes method={method} selected={scopes} onChange={setScopes} disabled={busy} />
          ) : null}
          {method.scopes.length > 0 && method.kind === "fields" ? (
            <TokenScopes method={method} mode={scopeMode} onMode={setScopeMode} selected={scopes} onChange={setScopes} disabled={busy} />
          ) : null}
          {method.kind === "oauth2" && method.client_key ? (
            <OAuthClientField value={clientId} onChange={setClientId} clients={matching} />
          ) : null}
        </>
      ) : null}
      {flowNotice ? <Notice tone={flowNotice.tone} role="alert">{flowNotice.text}</Notice> : null}
      {blocked ? (
        <Notice tone="pending" role="alert">
          Your browser blocked the sign-in window. Allow pop-ups for this site, then try again.
        </Notice>
      ) : null}
      {submitError ? <Notice tone="danger" role="alert">{errorMessage(submitError)}</Notice> : null}
      <FormActions>
        <Button type="submit" size="lg" disabled={busy || !method}>
          {busy
            ? "Connecting…"
            : method?.kind === "oauth2"
              ? blocked
                ? "Try again"
                : "Continue to sign in"
              : "Reconnect"}
        </Button>
      </FormActions>
    </Form>
  );
}

/**
 * "Give agents access" after a connect (D-137). A result that came over the channel (OAuth) carries no scopes, so the
 * connection is looked up in the refreshed list first.
 */
function ConnectedStep({
  app,
  workspaceId,
  orgId,
  methodId,
  done,
  onDismiss,
}: {
  app: AppDetail;
  workspaceId: string;
  orgId: string;
  methodId?: string;
  done: { connectionId: string; scopes?: string[] };
  onDismiss: () => void;
}) {
  const conns = useConnections({ kind: "workspace", id: workspaceId });
  const lookup = done.scopes === undefined;
  const found = lookup ? conns.data?.items.find((c) => c.id === done.connectionId) : undefined;
  const refetch = conns.refetch;
  useEffect(() => {
    if (lookup) void refetch();
  }, [lookup, refetch]);
  if (lookup && !found) return conns.isError ? <ErrorState error={conns.error} onRetry={() => void refetch()} /> : <Loading />;
  return (
    <GiveAccessPanel
      subject={{
        target: { kind: "connection", id: done.connectionId },
        scope: { kind: "workspace", id: workspaceId },
        orgId,
        appId: app.app_id,
        appWorkspaceId: workspaceId,
        connectMethod: found?.connect_method ?? methodId,
        scopes: done.scopes ?? found?.scopes ?? [],
        customOnly: appIsCustomOnly(app),
      }}
      onDismiss={onDismiss}
    />
  );
}

/** A reconnect opens on the method the connection was made with; wait for it (a failed load falls back). */
function ReconnectForm({ app, workspaceId, orgId, orgScope, reconnect, onClose }: { app: AppDetail; workspaceId: string; orgId: string; orgScope?: boolean; reconnect: string; onClose: () => void }) {
  const conns = useConnections(orgScope ? { kind: "organization", id: orgId } : { kind: "workspace", id: workspaceId });
  if (conns.isPending) return <Loading />;
  const keep = conns.data?.items.find((c) => c.id === reconnect)?.connect_method ?? null;
  return <ConnectForm app={app} workspaceId={workspaceId} orgId={orgId} orgScope={orgScope} keepMethod={keep} reconnect={reconnect} onClose={onClose} />;
}

/**
 * The Reconnect dialog (D-272): the whole flow (method, fields, scopes, submit, missing scopes, Give agents access)
 * happens in a `Dialog` on the page, like Details. Closing it (×, Escape, scrim, Done) drops the form. Connecting a new
 * account is the connect journey's job; this dialog is only mounted with a connection to reconnect.
 */
export function ConnectDialog({
  appId,
  name,
  workspaceId,
  orgId,
  orgScope,
  reconnect,
  onClose,
}: {
  appId: string;
  /** The app's name when the caller already knows it (shown in the title while the detail loads). */
  name?: string;
  workspaceId: string;
  orgId: string;
  /** (P5-UX-11) Reconnect an organization's account: nothing is read or written for `workspaceId`. */
  orgScope?: boolean;
  /** The id of the connection to reconnect. */
  reconnect: string;
  onClose: () => void;
}) {
  const detail = useApp(appId, orgScope ? undefined : workspaceId);
  const d = detail.data;
  const title = `Reconnect ${d ? appDisplayName(d) : (name ?? "app")}`;
  let body;
  if (detail.isPending) body = <Loading />;
  else if (detail.isError) body = <ErrorState error={detail.error} onRetry={() => void detail.refetch()} />;
  else if (!d) body = <Loading />;
  else if (!d.available) body = <ErrorState error={{ code: "app_unavailable" }} />;
  else if (d.connect.length > 1)
    body = <ReconnectForm app={d} workspaceId={workspaceId} orgId={orgId} orgScope={orgScope} reconnect={reconnect} onClose={onClose} />;
  else body = <ConnectForm app={d} workspaceId={workspaceId} orgId={orgId} orgScope={orgScope} reconnect={reconnect} onClose={onClose} />;
  return (
    <Dialog open onOpenChange={(o) => (o ? null : onClose())}>
      <DialogContent size="lg" title={title} description={d ? `${d.package_id} ${d.package_version}` : undefined}>
        {body}
      </DialogContent>
    </Dialog>
  );
}
