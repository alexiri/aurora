"use client";

import { useQueryClient } from "@tanstack/react-query";
import Link from "next/link";
import { useCallback, useEffect, useRef, useState, type FormEvent } from "react";

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
import {
  ackConnect,
  buildConnectFields,
  connectApp,
  defaultValues,
  navigateSignIn,
  openSignInWindow,
  useApp,
  useApps,
  useAppGuide,
  useAppInstances,
  appIsCustomOnly,
  useConnectChannel,
  useConnections,
  useOAuthClients,
  validateFields,
  type AppDetail,
  type AppSummary,
  type ConnectChannelMessage,
  type ConnectMethod,
  type FieldValues,
} from "@/lib/api/apps";

import { GiveAccessPanel } from "@/components/credentials/give-access";
import { ScopeGroups } from "@/components/grants/scope-groups";
import { Segmented } from "@/components/ui/segmented";

import { DynamicForm } from "./dynamic-form";
import { Markdown, safeHref } from "@/components/ui/markdown";

/**
 * (FIX-135, D-242) The first question for an app of a family: where does it run? The hosted
 * deployment (github.com), then every registered server of the family's instance-bound apps, then
 * how to register one. Each choice is one app (and instance); the gateway guarantee is unchanged.
 */
function DeploymentStep({
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
            <Link href="/admin/instances" className="font-medium underline">
              Register a server… (Admin › Self-hosted apps)
            </Link>
          ) : (
            "Ask an administrator to register one (Self-hosted apps)."
          )}
        </Notice>
      ) : bound.length > 0 && me.data?.user.is_instance_admin ? (
        <p className="text-xs text-muted">
          Another server?{" "}
          <Link href="/admin/instances" className="font-medium underline">
            Register a server… (Admin › Self-hosted apps)
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
  initialInstanceId,
  onClose,
}: {
  app: AppDetail;
  workspaceId: string;
  orgId: string;
  /** (FIX-135) The server already chosen in a family's "Where?" step: the instance picker is not asked again. */
  initialInstanceId?: string;
  /** A reconnect keeps the connection's method unless the person picks another (D-214). */
  keepMethod?: string | null;
  /** The id of the connection being reconnected. */
  reconnect?: string;
  onClose: () => void;
}) {
  const qc = useQueryClient();
  const first = initialMethod(app, keepMethod);
  // Instance-bound apps (D-74): the SaaS default host, or an instance an admin registered. A
  // reconnect keeps the connection's instance, so the picker is not shown then.
  const instanceBound = !!app.instances && !reconnect;
  const instances = useAppInstances(instanceBound ? { kind: "organization", id: orgId } : undefined);
  const [instanceId, setInstanceId] = useState(initialInstanceId ?? "");
  const choices = (instances.data?.items ?? []).filter((i) => i.app_id === app.app_id && i.status === "active");
  const builtinChoices = choices.filter((i) => i.builtin);
  const registeredChoices = choices.filter((i) => !i.builtin);
  const defaultHost = app.instances?.default_host ?? null;
  const [methodId, setMethodId] = useState(first?.id ?? "");
  const method: ConnectMethod | undefined = app.connect.find((m) => m.id === methodId);
  const [values, setValues] = useState<FieldValues>(() => defaultValues(first?.fields ?? []));
  const [scopes, setScopes] = useState<string[]>(first?.default_scopes ?? []);
  const [access, setAccess] = useState(first?.access_levels[0]?.id ?? "");
  // (FIX-136) A pasted token's scopes: detected by the method's read probes, or ticked by the person.
  const [scopeMode, setScopeMode] = useState<"detect" | "choose">("detect");
  // (FIX-133, D-240) What agents may do through the connection: only offered for an app that has write tools, not on a
  // reconnect (which keeps the connection's own choice). Read only is the recommended default.
  const hasWriteTools = app.tools.some((t) => t.group === "write");
  const [connAccess, setConnAccess] = useState<"read" | "write">("read");
  const [clientId, setClientId] = useState("");
  const [errors, setErrors] = useState<Record<string, string>>({});
  const [submitError, setSubmitError] = useState<unknown>(null);
  const [busy, setBusy] = useState(false);
  // `scopes` is unknown when the result came over the channel after an OAuth sign-in; the step looks it up.
  const [done, setDone] = useState<{ missing: string[]; connectionId: string; scopes?: string[] } | null>(null);
  const [accessDone, setAccessDone] = useState(false);
  // (C-842) OAuth: the provider's sign-in runs in a separate window while the dialog waits for its result.
  const [phase, setPhase] = useState<"form" | "waiting">("form");
  const [blocked, setBlocked] = useState(false);
  const [flowNotice, setFlowNotice] = useState<{ tone: "pending" | "danger"; text: string } | null>(null);
  const signIn = useRef<Window | null>(null);
  const signInUrl = useRef<string | null>(null);
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

  const refreshLists = useCallback(() => {
    void qc.invalidateQueries({ queryKey: ["apps"] });
    void qc.invalidateQueries({ queryKey: ["connections"] });
  }, [qc]);

  function closeSignIn() {
    signIn.current?.close();
    signIn.current = null;
  }

  // The dialog going away takes the sign-in window with it; nothing waits for a result any more.
  useEffect(() => () => closeSignIn(), []);

  // The callback page's result for the one flow this dialog is waiting for (C-842).
  const onResult = useCallback(
    (m: ConnectChannelMessage) => {
      if (phase !== "waiting") return;
      if (m.type === "connected") {
        if (m.appId !== app.app_id) return;
        ackConnect({ connectionId: m.connectionId });
        signIn.current = null;
        refreshLists();
        setDone({ missing: m.missingScopes, connectionId: m.connectionId });
        setPhase("form");
      } else {
        if (m.appId !== null && m.appId !== app.app_id) return;
        ackConnect({ error: m.error });
        signIn.current = null;
        setPhase("form");
        setFlowNotice({ tone: "danger", text: `The sign-in did not complete (${m.error}). Try again.` });
      }
    },
    [phase, app.app_id, refreshLists],
  );
  useConnectChannel(onResult);

  // A sign-in window closed without a result: back to the form.
  useEffect(() => {
    if (phase !== "waiting") return;
    const timer = setInterval(() => {
      if (signIn.current?.closed) {
        signIn.current = null;
        setPhase("form");
        setFlowNotice({ tone: "pending", text: "Sign-in was cancelled." });
      }
    }, 1000);
    return () => clearInterval(timer);
  }, [phase]);

  function reopenSignIn() {
    if (signIn.current && !signIn.current.closed) {
      signIn.current.focus();
      return;
    }
    const w = openSignInWindow();
    if (!w || !signInUrl.current) {
      w?.close();
      setBlocked(true);
      return;
    }
    navigateSignIn(w, signInUrl.current);
    signIn.current = w;
  }

  function cancelSignIn() {
    closeSignIn();
    setPhase("form");
    setFlowNotice(null);
  }

  function submit(e: FormEvent) {
    e.preventDefault();
    if (!method) return;
    const errs = validateFields(method.fields, values);
    if (instanceBound && !defaultHost && !instanceId) errs.__instance = "Choose an instance.";
    setErrors(errs);
    if (Object.keys(errs).length) return;
    setFlowNotice(null);
    setBlocked(false);
    // The sign-in window is opened here, synchronously in the click, so popup blockers allow it (C-842).
    let w: Window | null = null;
    if (method.kind === "oauth2") {
      w = openSignInWindow();
      if (!w) {
        setBlocked(true);
        return;
      }
      signIn.current = w;
    }
    void send(method, w);
  }

  async function send(method: ConnectMethod, w: Window | null) {
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
        ...(method.kind === "fields" && method.scopes.length > 0 && (scopeMode === "choose" || method.probe_scopes.length === 0)
          ? { scopes }
          : {}),
        ...(hasWriteTools && !reconnect ? { access: connAccess } : {}),
        ...(reconnect ? { reconnect_connection_id: reconnect } : {}),
      });
      if (res.kind === "redirect") {
        const url = safeHref(res.url);
        if (!url || !w) throw new Error("The server returned an invalid redirect.");
        // The provider's page must not get a handle on the dashboard window (reverse tabnabbing, WEBC-3).
        navigateSignIn(w, url);
        signInUrl.current = url;
        setPhase("waiting");
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
          <Notice tone="pending">Some requested scopes were not granted: {done.missing.join(", ")}.</Notice>
        ) : null}
        {!accessDone ? (
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
      {instanceBound && !initialInstanceId ? (
        <Field
          id="cm-instance"
          label={app.instances?.label ?? "Instance"}
          error={errors.__instance}
          hint={
            choices.length === 0 && !defaultHost
              ? "No self-hosted server is registered yet. Ask an organization admin to register one (Self-hosted apps)."
              : builtinChoices.length > 0
                ? "Credentials are only sent to the host you choose here, so pick the one your organization lives on. A self-hosted server must be registered first (Self-hosted apps)."
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
            {/* (FIX-134, D-243) The package's own preset instances (a vendor's regions), then servers an admin registered. */}
            {builtinChoices.map((i) => (
              <option key={i.id} value={i.id}>{`${i.label} (${i.host})`}</option>
            ))}
            {registeredChoices.length > 0 && builtinChoices.length > 0 ? (
              <optgroup label="Registered servers">
                {registeredChoices.map((i) => (
                  <option key={i.id} value={i.id}>{`${i.label} (${i.base_url})`}</option>
                ))}
              </optgroup>
            ) : (
              registeredChoices.map((i) => <option key={i.id} value={i.id}>{`${i.label} (${i.base_url})`}</option>)
            )}
          </select>
        </Field>
      ) : null}
      {needsInstanceClient ? (
        <Notice tone="pending" role="status">
          No OAuth application is registered for this self-hosted server yet. Ask an organization admin to add one under
          Credentials, OAuth clients, or connect with an access token instead.
        </Notice>
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
            <fieldset className="min-w-0 space-y-3">
              <legend className="mb-1 text-sm font-medium text-ink">Requested scopes</legend>
              <ScopeGroups scopes={method.scopes} selected={scopes} onChange={setScopes} disabled={busy} />
            </fieldset>
          ) : null}
          {method.scopes.length > 0 && method.kind === "fields" ? (
            <fieldset className="min-w-0 space-y-3">
              <legend className="mb-1 text-sm font-medium text-ink">Which scopes did you give this token?</legend>
              <p className="text-xs text-muted">
                A pasted token does not tell Aurora what it may do. Aurora uses this only to hide tools the token cannot serve
                (they would fail upstream); it never gives the token more than it has. You can change it later on the connection.
              </p>
              {method.probe_scopes.length > 0 ? (
                <Segmented<"detect" | "choose">
                  label="Token scopes"
                  value={scopeMode}
                  onChange={setScopeMode}
                  options={[
                    { value: "detect", label: "Detect read scopes" },
                    { value: "choose", label: "I will tick them" },
                  ]}
                />
              ) : null}
              {scopeMode === "choose" || method.probe_scopes.length === 0 ? (
                <ScopeGroups scopes={method.scopes} selected={scopes} onChange={setScopes} disabled={busy} />
              ) : (
                <p className="text-xs text-muted">
                  Aurora checks each read scope with a harmless read request using this token. Write scopes are never checked:
                  tick them yourself afterwards on the connection if the token has them.
                </p>
              )}
            </fieldset>
          ) : null}
          {hasWriteTools && !reconnect ? (
            <ChoiceCards
              legend="What may agents do with this connection?"
              value={connAccess}
              onChange={setConnAccess}
              options={[
                {
                  value: "read",
                  label: "Read only",
                  badge: "Recommended",
                  description:
                    "Agents can look things up but never change anything: Aurora blocks every write operation, whatever the token or the grants allow. You can change this later on the connection.",
                },
                {
                  value: "write",
                  label: "Read and write",
                  description: "Agents may also make changes, as far as the grants you give them allow.",
                },
              ]}
            />
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
              : reconnect
                ? "Reconnect"
                : "Connect"}
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
function ReconnectForm({ app, workspaceId, orgId, reconnect, onClose }: { app: AppDetail; workspaceId: string; orgId: string; reconnect: string; onClose: () => void }) {
  const conns = useConnections({ kind: "workspace", id: workspaceId });
  if (conns.isPending) return <Loading />;
  const keep = conns.data?.items.find((c) => c.id === reconnect)?.connect_method ?? null;
  return <ConnectForm app={app} workspaceId={workspaceId} orgId={orgId} keepMethod={keep} reconnect={reconnect} onClose={onClose} />;
}

/**
 * The Connect / Reconnect dialog (D-272): the whole flow (method, fields, scopes, submit, missing scopes, Give agents
 * access) happens in a `Dialog` on the page, like Details. Closing it (×, Escape, scrim, Done) drops the form.
 */
export function ConnectDialog({
  appId,
  name,
  workspaceId,
  orgId,
  reconnect,
  onClose,
}: {
  appId: string;
  /** The app's name when the caller already knows it (shown in the title while the detail loads). */
  name?: string;
  workspaceId: string;
  orgId: string;
  /** The id of the connection to reconnect. */
  reconnect?: string;
  onClose: () => void;
}) {
  // (FIX-135, D-242) An app of a family asks where the person's product runs first; the chosen
  // deployment is then the app (and, for a registered server, the instance) that is connected.
  const [where, setWhere] = useState<{ appId: string; instanceId?: string } | null>(null);
  const entry = useApp(appId, workspaceId);
  const apps = useApps(workspaceId);
  const family = entry.data?.family ?? null;
  const siblings = family ? (apps.data?.items ?? []).filter((a) => a.family?.id === family.id && a.available) : [];
  const asking = !reconnect && !where && siblings.length > 1;
  const shownId = where?.appId ?? appId;
  const detail = useApp(shownId, workspaceId);
  const d = detail.data;
  const product = family?.name ?? d?.name ?? name ?? "app";
  const title = `${reconnect ? "Reconnect" : "Connect"} ${asking || where ? product : (d ? appDisplayName(d) : (name ?? "app"))}`;
  let body;
  if (entry.isPending || (family && apps.isPending)) body = <Loading />;
  else if (asking) body = <DeploymentStep family={family!} siblings={siblings} orgId={orgId} onPick={setWhere} />;
  else if (detail.isPending) body = <Loading />;
  else if (detail.isError) body = <ErrorState error={detail.error} onRetry={() => void detail.refetch()} />;
  else if (!d) body = <Loading />;
  else if (!d.available) body = <ErrorState error={{ code: "app_unavailable" }} />;
  else if (reconnect && d.connect.length > 1)
    body = <ReconnectForm app={d} workspaceId={workspaceId} orgId={orgId} reconnect={reconnect} onClose={onClose} />;
  else
    body = (
      <ConnectForm
        key={`${shownId}:${where?.instanceId ?? ""}`}
        app={d}
        workspaceId={workspaceId}
        orgId={orgId}
        reconnect={reconnect}
        initialInstanceId={where?.instanceId}
        onClose={onClose}
      />
    );
  return (
    <Dialog open onOpenChange={(o) => (o ? null : onClose())}>
      <DialogContent size="lg" title={title} description={d ? `${d.package_id} ${d.package_version}` : undefined}>
        {body}
      </DialogContent>
    </Dialog>
  );
}
