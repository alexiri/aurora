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
import { Form, FormActions } from "@/components/ui/form";
import { Markdown, safeHref } from "@/components/ui/markdown";
import { Notice } from "@/components/ui/notice";
import { WhoCan } from "@/components/ui/who-can";
import { accountKeys, type Level } from "@/lib/api/accounts";
import {
  connectApp,
  defaultValues,
  useAppGuide,
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
import { askSentence, onlySentence, type WhoCanValue } from "@/lib/who-can";
import { useParams } from "next/navigation";

import { WebhookSignInStep } from "@/components/webhook/webhook-connect-step";

import { DynamicForm } from "../dynamic-form";
import { InstanceField, OAuthClientField, RequestedScopes, TokenScopes } from "./advanced-fields";
import { buildJourneyConnect, connectFailure, hostOf, suggestedMethod, tickList, tokenPage } from "./logic";
import { useSignInWindow } from "./use-sign-in-window";

/** What the step reports when the account exists. `granted` are the agents the access was saved for. */
export interface SignInResult {
  accountId: string;
  granted: string[];
  grantFailed: string | null;
  missing: string[];
  /** The scopes the new account has and the method it was made with: the Access dialog lists tools the scopes cannot serve (FIX-136). */
  scopes?: string[];
  methodId?: string;
}

const reason = (w: WhoCanValue | null | undefined) =>
  w ? `${onlySentence(w)}${w.people.length > 0 ? ` ${askSentence(w, false)}` : ""}` : "Not set up yet.";

/** The values the person must copy into the other service (C-942), above the method's fields. */
function ShowContext({ method, orgId }: { method: ConnectMethod; orgId: string }) {
  const wants = method.show_context.includes("org_external_id");
  const org = useOrg(wants ? orgId : undefined);
  const id = org.data?.external_id;
  if (!wants || !id) return null;
  return <CopyValue label="Your organization's external id" value={id} hint="Not a secret. Paste it into the role's trust policy." />;
}

/**
 * **Sign in** (`14 §3` step 4): only the suggested method, with one primary action; an **Advanced** disclosure holds the
 * other methods (blocked ones turned off, with the reason), the scopes, the OAuth client and a self-hosted server.
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
      onResult({
        accountId: res.connection.id,
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
      setBusy(false);
      // Secrets never linger in state after a submit attempt.
      setValues((v) => {
        const next = { ...v };
        for (const f of method.fields) if (f.type === "secret") next[f.name] = "";
        return next;
      });
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
  const ticks = method && method.kind === "fields" ? tickList(method, level) : [];
  const family = app.family;

  return (
    <Form onSubmit={(e) => void submit(e)} aria-label={`Sign in to ${app.name}`} noValidate>
      {method ? (
        <>
          {app.connect_note && !help ? <Notice>{app.connect_note.text}</Notice> : null}
          {method.kind === "fields" && help ? (
            <div className="space-y-2">
              <p className="text-sm text-ink">Paste {app.name}&apos;s access token.</p>
              <a className={buttonVariants({ variant: "outline", size: "sm" })} href={help} target="_blank" rel="noopener noreferrer">
                Create a token on {app.name}
              </a>
              {ticks.length > 0 ? (
                <p className="text-sm text-ink-2">
                  {level === "read" || level === null ? "For Read only, tick" : "Tick"}: <span className="font-medium text-ink">{ticks.join(", ")}</span>
                </p>
              ) : null}
            </div>
          ) : null}
          {method.kind === "oauth2" ? <p className="text-sm text-ink-2">A sign-in window opens at {app.name}. Nothing is shared until you approve.</p> : null}
          <ShowContext method={method} orgId={orgId} />
          <DynamicForm fields={fields} values={values} errors={errors} onChange={(n, v) => setValues((s) => ({ ...s, [n]: v }))} />
        </>
      ) : (
        <div className="space-y-1">
          <p className="text-sm font-medium text-ink">{app.name} cannot be connected yet.</p>
          {blockedWho ? <WhoCan value={blockedWho} lead /> : <p className="text-sm text-ink-2">Sign-in is not set up for {app.name}.</p>}
        </div>
      )}

      <details className="rounded-control border border-line p-3">
        <summary className="min-h-9 cursor-pointer text-sm font-semibold text-ink">Advanced: other ways to connect, scopes</summary>
        <div className="mt-3 space-y-4">
          {app.connect.length > 1 ? (
            <ChoiceCards
              legend="How do you want to connect?"
              value={method?.id ?? ""}
              onChange={pick}
              options={app.connect.map((m) => {
                const setup = plan.methods.find((x) => x.id === m.id);
                const blocked = setup ? !setup.ready : false;
                return {
                  value: m.id,
                  label: m.label ?? m.id,
                  description: blocked ? reason(setup?.who_can) : (m.description ?? undefined),
                  badge: m.id === plan.suggested_method ? "Suggested" : undefined,
                  disabled: blocked,
                };
              })}
            />
          ) : null}
          {method && method.scopes.length > 0 && method.kind !== "fields" ? (
            <RequestedScopes method={method} selected={scopes ?? method.default_scopes} onChange={setScopes} disabled={busy} />
          ) : null}
          {method && method.scopes.length > 0 && method.kind === "fields" ? (
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
          {method?.kind === "oauth2" && method.client_key ? <OAuthClientField value={clientId} onChange={setClientId} clients={matching} /> : null}
          {family && !family.instance_bound ? (
            <p className="text-sm">
              <Link href={wsRoutes(org, ws).app(app.app_id)} className="font-medium text-link underline">
                Use a self-hosted {family.name}
              </Link>
            </p>
          ) : null}
          {guide.data ? (
            <details className="rounded-control border border-line p-3">
              <summary className="min-h-9 cursor-pointer text-sm font-semibold text-ink">Setup guide</summary>
              <div className="mt-2"><Markdown source={guide.data} /></div>
            </details>
          ) : null}
        </div>
      </details>

      {signIn.notice ? <Notice tone={signIn.notice.tone} role="alert">{signIn.notice.text}</Notice> : null}
      {signIn.blocked ? (
        <Notice tone="pending" role="alert">
          Your browser blocked the sign-in window. Allow pop-ups for this site, then try again.
        </Notice>
      ) : null}
      {error ? <Notice tone="danger" role="alert">{connectFailure(error, app.name)}</Notice> : null}
      <FormActions>
        {onBack ? (
          <Button type="button" variant="ghost" onClick={onBack}>
            Back
          </Button>
        ) : null}
        <Button type="submit" size="lg" disabled={busy || !method}>
          {busy ? "Connecting…" : signIn.blocked ? "Try again" : verb}
        </Button>
      </FormActions>
    </Form>
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
      onResult({ accountId: made.id, granted: agentIds, grantFailed: null, missing: [], name: made.name });
    } catch {
      // The name and the website stay; the token does not.
      setValue("");
    }
  }

  return (
    <Form onSubmit={(e) => void submit(e)} aria-label="Another website or API" noValidate>
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
      <FormActions>
        {onBack ? (
          <Button type="button" variant="ghost" onClick={onBack}>
            Back
          </Button>
        ) : null}
        <Button type="submit" size="lg" disabled={create.isPending}>
          {create.isPending ? "Connecting…" : "Connect"}
        </Button>
      </FormActions>
    </Form>
  );
}
