"use client";

import { useQueryClient } from "@tanstack/react-query";
import Link from "next/link";
import { useCallback, useState, type ComponentProps, type FormEvent } from "react";

import { RecipeFieldsInput } from "@/components/credentials/generic-form";
import { ValueInput } from "@/components/credentials/value-input";
import { Button, buttonVariants } from "@/components/ui/button";
import { ChoiceCards } from "@/components/ui/choice-cards";
import { CopyValue } from "@/components/ui/copy-value";
import { Field, inputClass } from "@/components/ui/field";
import { Form } from "@/components/ui/form";
import { LinkIcon } from "@/components/ui/icons";
import { safeHref } from "@/components/ui/markdown";
import { Notice } from "@/components/ui/notice";
import { WhoCan } from "@/components/ui/who-can";
import { LEVEL_LABEL } from "@/lib/accounts";
import { accountKeys, type Level } from "@/lib/api/accounts";
import { useAskSetup, useWithdrawSetup } from "@/lib/api/app-setup-requests";
import {
  connectApp,
  defaultValues,
  useAppGuide,
  useApps,
  useAppInstances,
  useOAuthClients,
  validateFields,
  type AppDetail,
  type ConnectMethod,
  type ConnectPlan,
  type FieldValues,
} from "@/lib/api/apps";
import { errorMessage, fieldErrors } from "@/lib/api/errors";
import {
  buildGenericCreate,
  EMPTY_RECIPE_FIELDS,
  useCreateSecret,
  type GenericFormState,
  type RecipeFields,
  type RecipeKind,
} from "@/lib/api/secrets";
import { useOrg } from "@/lib/api/queries";
import { wsRoutes } from "@/lib/routes";
import type { WhoCanValue } from "@/lib/who-can";
import { useParams } from "next/navigation";

import { WebhookSignInStep } from "@/components/webhook/webhook-connect-step";

import { DynamicForm } from "../dynamic-form";
import { InstanceField, OAuthClientField, RequestedScopes, TokenScopes } from "./advanced-fields";
import { BackNext, StepFooter } from "./frame";
import { buildJourneyConnect, connectFailure, firstName, hostOf, permissionHints, suggestedMethod, tokenPage } from "./logic";
import { useSignInWindow } from "./use-sign-in-window";

/** What the step reports when the account exists. `granted` are the agents the access was saved for. */
export interface SignInResult {
  accountId: string;
  /** The account's name, when the connect answer carries it (Done names it). */
  accountName?: string;
  granted: string[];
  grantFailed: string | null;
  missing: string[];
  /** The scopes the new account has and the method it was made with: the Access dialog lists tools the scopes cannot serve (FIX-136). */
  scopes?: string[];
  methodId?: string;
}

/** The values the person must copy into the other service (C-942), above the method's fields. */
function ShowContext({ method, orgId }: { method: ConnectMethod; orgId: string }) {
  const wants = method.show_context.includes("org_external_id");
  const org = useOrg(wants ? orgId : undefined);
  const id = org.data?.external_id;
  if (!wants || !id) return null;
  return <CopyValue label="Your organization's external id" value={id} hint="Not a secret. Paste it into the role's trust policy." />;
}

/**
 * **Sign in** (`14 §3` step 4, P5-POLISH-5, B4): the suggested method with numbered steps for a token (create it, the
 * permissions to give it for the chosen access, paste it); a choice at the top only when several ways work; **More
 * options** holds the scopes, the OAuth client and a self-hosted server. A failure is said under its field, with the value kept.
 */
export function SignInStep(props: ComponentProps<typeof GenericSignInStep>) {
  // (P5-WEBHOOK-4, C-913) The Outgoing webhook app's sign-in is its three connect steps.
  if (props.app.app_id === "webhook")
    return (
      <WebhookSignInStep
        app={props.app}
        workspaceId={props.workspaceId}
        orgId={props.orgId}
        orgMode={props.orgMode}
        level={props.level}
        agentIds={props.agentIds}
        hasWriteTools={props.hasWriteTools}
        onBack={props.onBack}
        onResult={props.onResult}
      />
    );
  return <GenericSignInStep {...props} />;
}

function GenericSignInStep({
  app,
  plan,
  workspaceId,
  orgId,
  level,
  agentIds,
  fixedInstanceId,
  hasWriteTools,
  orgMode,
  onBack,
  onResult,
}: {
  app: AppDetail;
  plan: ConnectPlan;
  workspaceId: string;
  /** (P5-UX-11) The organization's journey: the account is for the whole organization and `level` is its limit. */
  orgMode?: boolean;
  orgId: string;
  level: Level | null;
  agentIds: string[];
  fixedInstanceId?: string;
  hasWriteTools: boolean;
  onBack?: () => void;
  onResult: (r: SignInResult) => void;
}) {
  const qc = useQueryClient();
  const { org, ws } = useParams<{ org: string; ws: string }>();
  const suggested = suggestedMethod(plan, app);
  const [chosen, setChosen] = useState<string | null>(null);
  const method: ConnectMethod | undefined = chosen ? app.connect.find((m) => m.id === chosen) : suggested?.method;
  const [values, setValues] = useState<FieldValues>(() => defaultValues(method?.fields ?? []));
  const [errors, setErrors] = useState<Record<string, string>>({});
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<unknown>(null);
  const [scopes, setScopes] = useState<string[] | null>(null);
  const [scopeMode, setScopeMode] = useState<"detect" | "choose">("detect");
  const [clientId, setClientId] = useState("");
  const [instanceId, setInstanceId] = useState(fixedInstanceId ?? "");

  const instanceBound = !!app.instances;
  const instances = useAppInstances(instanceBound && !fixedInstanceId ? { kind: "organization", id: orgId } : undefined);
  const choices = (instances.data?.items ?? []).filter((i) => i.app_id === app.app_id && i.status === "active");
  const clients = useOAuthClients(orgMode ? { kind: "organization", id: orgId } : { kind: "workspace", id: workspaceId });
  const matching = (clients.data?.items ?? []).filter(
    (c) =>
      c.enabled &&
      c.package_id === app.package_id &&
      c.client_key === method?.client_key &&
      (instanceBound ? (c.instance_id ?? "") === instanceId : !c.instance_id),
  );

  const refresh = useCallback(() => {
    void qc.invalidateQueries({ queryKey: ["apps"] });
    void qc.invalidateQueries({ queryKey: ["connections"] });
    void qc.invalidateQueries({ queryKey: accountKeys.all });
  }, [qc]);

  const signIn = useSignInWindow(
    app.app_id,
    useCallback(
      (m) => {
        refresh();
        onResult({ accountId: m.connectionId, granted: m.grantedAgentIds, grantFailed: m.grantFailed, missing: m.missingScopes, methodId: method?.id });
      },
      [refresh, onResult, method?.id],
    ),
  );

  const guide = useAppGuide(app.app_id);

  function pick(id: string) {
    const m = app.connect.find((x) => x.id === id);
    setChosen(id);
    setValues(defaultValues(m?.fields ?? []));
    setScopes(null);
    setErrors({});
  }

  async function submit(e: FormEvent) {
    e.preventDefault();
    if (!method) return;
    const errs = validateFields(method.fields, values);
    if (instanceBound && !app.instances?.default_host && !instanceId) errs.__instance = "Choose an instance.";
    setErrors(errs);
    if (Object.keys(errs).length) return;
    // The sign-in window is opened here, synchronously in the click, so pop-up blockers allow it (C-842).
    let w: Window | null = null;
    if (method.kind === "oauth2") {
      w = signIn.open();
      if (!w) return;
    } else {
      signIn.setNotice(null);
      signIn.setBlocked(false);
    }
    setBusy(true);
    setError(null);
    try {
      const tokenScopes =
        method.kind === "fields" && method.scopes.length > 0 && scopeMode === "choose" ? (scopes ?? undefined) : undefined;
      const res = await connectApp(
        app.app_id,
        buildJourneyConnect({
          workspaceId,
          method,
          fields: values,
          agentIds,
          level,
          hasWriteTools,
          org: orgMode ? { orgId } : undefined,
          scopesOverride: method.kind === "fields" ? null : scopes,
          scopes: tokenScopes ?? null,
          instanceId: instanceBound && instanceId ? instanceId : undefined,
          clientId: clientId || undefined,
          accessLevel: method.access_levels[0]?.id,
        }),
      );
      if (res.kind === "redirect") {
        const url = safeHref(res.url);
        if (!url || !w) throw new Error("The server returned an invalid redirect.");
        signIn.navigate(w, url);
        return;
      }
      signIn.close();
      refresh();
      // Secrets live in this component's state only and go when the account exists (or the dialog closes).
      setValues(defaultValues([]));
      onResult({
        accountId: res.connection.id,
        accountName: res.connection.label ?? undefined,
        granted: res.grants.map((g) => g.agent_id),
        grantFailed: null,
        missing: res.missing_scopes,
        scopes: res.connection.scopes,
        methodId: method.id,
      });
    } catch (err) {
      signIn.close();
      setError(err);
    } finally {
      // (P5-POLISH-5, D-388) Typed values survive a failed connect, secrets included: they stay in this component's
      // state (masked, with Show), never in storage or the URL, and are dropped when the dialog closes.
      setBusy(false);
    }
  }

  if (signIn.phase === "waiting")
    return (
      <div role="status" className="space-y-3 text-sm">
        <p className="font-medium text-ink">Waiting for you to sign in to {app.name}…</p>
        <p className="text-ink-2">Finish in the sign-in window. This dialog continues when you are done.</p>
        {signIn.blocked ? (
          <Notice tone="pending" role="alert">
            Your browser blocked the sign-in window. Allow pop-ups for this site, then open it again.
          </Notice>
        ) : null}
        <div className="flex flex-wrap gap-2">
          <Button size="sm" variant="outline" onClick={signIn.reopen}>Open the sign-in window again</Button>
          <Button size="sm" variant="ghost" onClick={signIn.cancel}>Cancel</Button>
        </div>
      </div>
    );

  const help = method ? tokenPage(app, method) : null;
  // The token page is the button; the first field's own link would repeat it.
  const fields = method ? method.fields.map((f) => (f.help_url && safeHref(f.help_url) === help ? { ...f, help_url: null } : f)) : [];
  const blockedWho = plan.available_who_can ?? plan.methods.find((m) => m.who_can)?.who_can ?? null;
  const verb = method?.kind === "oauth2" ? `Sign in to ${app.name}` : "Connect";
  const hints = method && method.kind === "fields" ? permissionHints(method, level) : [];
  const family = app.family;
  // More than one way that works for people: the choice sits at the top of the step.
  const usable = plan.methods.filter((m) => m.ready && m.audience !== "admin");
  const secretField = method?.fields.find((f) => f.type === "secret")?.name;
  // A failed connect is said under the field it concerns, with the value kept (B4); other kinds below the form.
  const failure = error ? connectFailure(error, app.name) : null;
  const underField = !!failure && method?.kind === "fields" && !!secretField;
  const shownErrors = underField ? { ...errors, [secretField!]: failure! } : errors;
  const guideHref = ws ? wsRoutes(org, ws).app(app.app_id) : `/o/${org}/apps?tab=apps&app=${encodeURIComponent(app.app_id)}`;
  const moreOptions =
    (method && method.scopes.length > 0) ||
    (instanceBound && !fixedInstanceId) ||
    (method?.kind === "oauth2" && !!method.client_key) ||
    (family && !family.instance_bound);
  const needsInstance = instanceBound && !fixedInstanceId && !app.instances?.default_host;

  // A way in that nobody here can use yet: the same ask as the App step, not a disabled Connect (B2).
  if (!method)
    return <NeedsAdmin app={app} who={blockedWho} workspaceId={workspaceId} onBack={onBack} />;

  return (
    <Form id="journey-signin" onSubmit={(e) => void submit(e)} aria-label={`Sign in to ${app.name}`} noValidate>
      {family && (family.instance_bound || app.instances) ? <p className="text-xs text-muted">{family.name} · {family.variant_label}</p> : null}
      {usable.length > 1 ? (
        <ChoiceCards
          legend="Sign in with:"
          value={method.id}
          onChange={pick}
          options={app.connect
            .filter((m) => usable.some((u) => u.id === m.id))
            .map((m) => ({
              value: m.id,
              label: m.label ?? m.id,
              description: m.description ?? undefined,
              badge: m.id === plan.suggested_method ? "Suggested" : undefined,
            }))}
        />
      ) : null}
      {app.connect_note && !help ? <Notice>{app.connect_note.text}</Notice> : null}
      {method.kind === "fields" && help ? (
        <ol className="space-y-4 text-sm text-ink">
          <li>
            <p className="flex flex-wrap items-center gap-x-3 gap-y-2">
              <span>
                <b>1.</b> Create a token on {app.name}
              </span>
              <a className={buttonVariants({ variant: "outline", size: "sm" })} href={help} target="_blank" rel="noopener noreferrer">
                Open {app.name}
                <LinkIcon className="size-4" aria-hidden="true" />
              </a>
            </p>
          </li>
          {hints.length > 0 ? (
            <li className="space-y-2">
              <p>
                <b>2.</b> Give it these permissions, for <b>{level ? LEVEL_LABEL[level] : "Read only"}</b>:
              </p>
              <pre aria-label="Permissions to give the token" className="whitespace-pre-wrap rounded-control border border-line bg-subtle p-3 font-mono text-xs text-ink">
                {hints.join("\n")}
              </pre>
            </li>
          ) : null}
          <li>
            <p>
              <b>{hints.length > 0 ? "3." : "2."}</b> Paste the token
            </p>
          </li>
        </ol>
      ) : null}
      {method.kind === "oauth2" ? <p className="text-sm text-ink-2">A sign-in window opens at {app.name}. Nothing is shared until you approve.</p> : null}
      <ShowContext method={method} orgId={orgId} />
      <DynamicForm fields={fields} values={values} errors={shownErrors} onChange={(n, v) => setValues((s) => ({ ...s, [n]: v }))} />

      {moreOptions ? (
        <details open={needsInstance || undefined} className="rounded-control border border-line p-3">
          <summary className="min-h-9 cursor-pointer text-sm font-semibold text-ink">More options</summary>
          <div className="mt-3 space-y-4">
            {method.scopes.length > 0 && method.kind !== "fields" ? (
              <RequestedScopes method={method} selected={scopes ?? method.default_scopes} onChange={setScopes} disabled={busy} />
            ) : null}
            {method.scopes.length > 0 && method.kind === "fields" ? (
              <TokenScopes method={method} mode={scopeMode} onMode={setScopeMode} selected={scopes ?? method.default_scopes} onChange={setScopes} disabled={busy} />
            ) : null}
            {instanceBound && !fixedInstanceId ? (
              <InstanceField
                label={app.instances?.label ?? "Instance"}
                error={errors.__instance}
                value={instanceId}
                onChange={(v) => {
                  setInstanceId(v);
                  setClientId("");
                }}
                defaultHost={app.instances?.default_host ?? null}
                builtin={choices.filter((i) => i.builtin)}
                registered={choices.filter((i) => !i.builtin)}
              />
            ) : null}
            {method.kind === "oauth2" && method.client_key ? <OAuthClientField value={clientId} onChange={setClientId} clients={matching} /> : null}
            {family && !family.instance_bound ? (
              <p className="text-sm">
                <Link href={wsRoutes(org, ws).app(app.app_id)} className="font-medium text-link underline">
                  Use a self-hosted {family.name}
                </Link>
              </p>
            ) : null}
          </div>
        </details>
      ) : null}

      {signIn.notice ? <Notice tone={signIn.notice.tone} role="alert">{signIn.notice.text}</Notice> : null}
      {signIn.blocked ? (
        <Notice tone="pending" role="alert">
          Your browser blocked the sign-in window. Allow pop-ups for this site, then try again.
        </Notice>
      ) : null}
      {failure && !underField ? <Notice tone="danger" role="alert">{failure}</Notice> : null}
      {guide.data ? (
        <p className="text-sm">
          <Link href={guideHref} target="_blank" rel="noopener noreferrer" className="font-medium text-link underline">
            Step-by-step guide
          </Link>
        </p>
      ) : null}
      <BackNext form="journey-signin" onBack={onBack} nextLabel={signIn.blocked ? "Try again" : verb} busy={busy} busyLabel="Connecting…" />
    </Form>
  );
}

/**
 * (P5-POLISH-5, D-386, B2) The Sign in step of an app nobody here can sign in to yet (a deep link): the reason, who can fix
 * it and **Ask <Name>**; once asked, the asked state with **Undo**. Not a disabled Connect.
 */
function NeedsAdmin({ app, who, workspaceId, onBack }: { app: AppDetail; who: WhoCanValue | null; workspaceId: string; onBack?: () => void }) {
  const apps = useApps(workspaceId);
  const ask = useAskSetup(workspaceId);
  const withdraw = useWithdrawSetup(workspaceId);
  const [local, setLocal] = useState<"now" | "no" | null>(null);
  const mine = local ? local === "now" : !!apps.data?.items.find((a) => a.app_id === app.app_id)?.readiness?.requested?.by_me;
  const name = firstName(who?.people[0]?.name);
  return (
    <div className="space-y-3 text-sm">
      <p className="font-medium text-ink">{app.name} needs an admin to set it up first.</p>
      {who ? <WhoCan value={who} lead /> : <p className="text-ink-2">Sign-in is not set up for {app.name}.</p>}
      {mine ? (
        <p className="flex items-start gap-2 text-ink-2">
          <span aria-hidden="true" className="mt-1.5 size-2 shrink-0 rounded-full bg-accent" />
          <span>Asked{local === "now" ? " just now" : ""}. You&apos;ll get a notice when it&apos;s ready.</span>
        </p>
      ) : null}
      <StepFooter>
        {onBack ? (
          <Button type="button" variant="ghost" onClick={onBack}>
            Back
          </Button>
        ) : null}
        {mine ? (
          <Button type="button" variant="ghost" className="ml-auto" disabled={withdraw.isPending} onClick={() => withdraw.mutate(app.app_id, { onSuccess: () => setLocal("no") })}>
            Undo
          </Button>
        ) : (
          <Button type="button" size="lg" className="ml-auto" disabled={ask.isPending} onClick={() => ask.mutate(app.app_id, { onSuccess: () => setLocal("now") })}>
            {name ? `Ask ${name}` : "Ask an admin"}
          </Button>
        )}
      </StepFooter>
    </div>
  );
}

/**
 * **Another website or API** (`14 §3`): Name, Website and Token; **Advanced** holds the path and the other ways to send
 * the token (`08 §5`). Saved with the agents' access in one request (C-988).
 */
export function WebsiteStep({
  workspaceId,
  agentIds,
  onBack,
  onResult,
}: {
  workspaceId: string;
  agentIds: string[];
  onBack?: () => void;
  onResult: (r: SignInResult & { name: string }) => void;
}) {
  const create = useCreateSecret({ kind: "workspace", id: workspaceId });
  const [name, setName] = useState("");
  const [site, setSite] = useState("");
  const [value, setValue] = useState("");
  const [path, setPath] = useState("");
  const [kind, setKind] = useState<RecipeKind>("bearer");
  const [fields, setFields] = useState<RecipeFields>(EMPTY_RECIPE_FIELDS);
  const [clientErr, setClientErr] = useState("");
  const fe = create.error ? fieldErrors(create.error) : {};

  async function submit(e: FormEvent) {
    e.preventDefault();
    if (!name.trim() || !site.trim()) return setClientErr("Enter a name and a website.");
    if (!value) return setClientErr("Enter the token.");
    setClientErr("");
    const state: GenericFormState = {
      name,
      host: hostOf(site),
      path,
      kind,
      fields,
      source: "value",
      value,
      ref: { store_id: "", path: "", field: "" },
      agentIds,
    };
    try {
      const made = await create.mutateAsync(buildGenericCreate(state));
      setValue("");
      onResult({ accountId: made.id, accountName: made.name, granted: agentIds, grantFailed: null, missing: [], name: made.name });
    } catch {
      // The name, the website and the token all stay (masked); the error is said under the field.
    }
  }

  return (
    <Form id="journey-signin" onSubmit={(e) => void submit(e)} aria-label="Another website or API" noValidate>
      <Field id="jw-name" label="Name" hint="So you can tell accounts apart." error={fe.name}>
        <input id="jw-name" className={inputClass} value={name} onChange={(e) => setName(e.target.value)} />
      </Field>
      <Field id="jw-site" label="Website" hint="For example api.example.com" error={fe.host_pattern}>
        <input id="jw-site" className={inputClass} value={site} onChange={(e) => setSite(e.target.value)} />
      </Field>
      <ValueInput id="jw-token" label="Token" value={value} onChange={setValue} error={fe.value} />
      <details className="rounded-control border border-line p-3">
        <summary className="min-h-9 cursor-pointer text-sm font-semibold text-ink">Advanced: path, other ways to send the token</summary>
        <div className="mt-3 space-y-4">
          <Field id="jw-path" label="Path (optional)" error={fe.path_pattern}>
            <input id="jw-path" className={inputClass} value={path} onChange={(e) => setPath(e.target.value)} />
          </Field>
          <RecipeFieldsInput idp="jw" kind={kind} fields={fields} onKind={setKind} onFields={setFields} />
        </div>
      </details>
      {clientErr || create.error ? (
        <Notice tone="danger" role="alert">
          {clientErr || errorMessage(create.error)}
        </Notice>
      ) : null}
      <BackNext form="journey-signin" onBack={onBack} nextLabel="Connect" busy={create.isPending} busyLabel="Connecting…" />
    </Form>
  );
}
