"use client";

import { useState, type FormEvent } from "react";

import { EmptyState, ErrorState, Loading } from "@/components/states";
import { Button } from "@/components/ui/button";
import { Field, inputClass } from "@/components/ui/field";
import { errorMessage } from "@/lib/api/errors";
import {
  buildClientPatchFields,
  useAppInstances,
  useApps,
  useCreateOAuthClient,
  useDeleteOAuthClient,
  useOAuthClients,
  usePatchOAuthClient,
  type AppInstance,
  type CredScope,
  type InstanceScope,
  type OAuthClient,
  type OAuthClientDefinition,
} from "@/lib/api/apps";
import { useQueries } from "@tanstack/react-query";
import { api } from "@/lib/api/client";
import { unwrap } from "@/lib/api/unwrap";

interface Definition {
  package_id: string;
  app_name: string;
  def: OAuthClientDefinition;
  /** The app can be served by self-hosted instances (D-74): a client may be bound to one. */
  instance_bound: boolean;
  default_host: string | null;
}

/** Definitions (fields, setup note, redirect URI) come from AppDetail.oauth_clients; no app knowledge lives in the bundle. */
function useDefinitions(scope: CredScope) {
  const apps = useApps(scope.kind === "workspace" ? scope.id : undefined);
  const detailQueries = useQueries({
    queries: (apps.data?.items ?? []).map((a) => ({
      queryKey: ["app", a.app_id, ""],
      queryFn: () => unwrap(api.GET("/v1/apps/{app}", { params: { path: { app: a.app_id } } })),
      retry: false,
    })),
  });
  const defs = new Map<string, Definition>();
  for (const q of detailQueries)
    for (const d of q.data?.oauth_clients ?? [])
      defs.set(`${q.data!.package_id}|${d.client_key}`, {
        package_id: q.data!.package_id,
        app_name: q.data!.name,
        def: d,
        instance_bound: q.data!.instances != null,
        default_host: q.data!.instances?.default_host ?? null,
      });
  return defs;
}

function RedirectUri({ uri }: { uri: string }) {
  const [copied, setCopied] = useState(false);
  return (
    <div className="flex items-center gap-2 text-xs">
      <span className="font-medium">Redirect URI:</span>
      <code className="break-all rounded bg-neutral-100 px-1.5 py-0.5">{uri}</code>
      <Button
        type="button"
        size="sm"
        variant="outline"
        onClick={() => {
          void navigator.clipboard?.writeText(uri).then(() => setCopied(true));
        }}
      >
        {copied ? "Copied" : "Copy"}
      </Button>
    </div>
  );
}

/** One input per manifest-defined field; secrets are password inputs and never prefilled. */
function DefinitionInputs({
  def,
  values,
  onChange,
  existing,
}: {
  def: OAuthClientDefinition;
  values: Record<string, string>;
  onChange: (v: Record<string, string>) => void;
  existing?: boolean;
}) {
  return (
    <div className="space-y-2">
      {def.setup_note ? <p className="text-sm text-neutral-600">{def.setup_note}</p> : null}
      <RedirectUri uri={def.redirect_uri} />
      {def.fields.map((f) => {
        const id = `ocf-${def.client_key}-${f.name}`;
        return (
          <Field key={f.name} id={id} label={f.label}>
            <input
              id={id}
              className={inputClass}
              type={f.secret ? "password" : "text"}
              autoComplete="off"
              placeholder={existing && f.secret ? "Leave blank to keep the stored value" : ""}
              value={values[f.name] ?? ""}
              onChange={(e) => onChange({ ...values, [f.name]: e.target.value })}
            />
          </Field>
        );
      })}
    </div>
  );
}

function CreateForm({
  scope,
  onDone,
  defs,
  instances,
}: {
  scope: CredScope;
  onDone: () => void;
  defs: Map<string, Definition>;
  instances: AppInstance[];
}) {
  const create = useCreateOAuthClient(scope);
  const [choice, setChoice] = useState("");
  const [instanceId, setInstanceId] = useState("");
  const [values, setValues] = useState<Record<string, string>>({});
  const selected = defs.get(choice);
  // An instance-level client binds instance-wide instances only; the others also their org's own.
  const choices = instances.filter(
    (i) => i.package_id === selected?.package_id && i.status === "active" && (scope.kind !== "instance" || i.org_id == null),
  );
  async function submit(e: FormEvent) {
    e.preventDefault();
    if (!selected) return;
    const fields: Record<string, string> = {};
    for (const f of selected.def.fields) if ((values[f.name] ?? "") !== "") fields[f.name] = values[f.name];
    try {
      await create.mutateAsync({
        package_id: selected.package_id,
        client_key: selected.def.client_key,
        fields,
        ...(selected.instance_bound && instanceId ? { instance_id: instanceId } : {}),
      });
      setValues({});
      setInstanceId("");
      onDone();
    } catch {
      // shown below
    }
  }
  return (
    <form onSubmit={(e) => void submit(e)} className="space-y-3 rounded-md border border-neutral-200 p-3" aria-label="Add OAuth client">
      <Field id="oc-app" label="App">
        <select
          id="oc-app"
          className={inputClass}
          value={choice}
          onChange={(e) => {
            setChoice(e.target.value);
            setInstanceId("");
            setValues({});
          }}
          required
        >
          <option value="">Select…</option>
          {[...defs.entries()].map(([k, o]) => (
            <option key={k} value={k}>{`${o.app_name} (${o.def.client_key})`}</option>
          ))}
        </select>
      </Field>
      {selected?.instance_bound ? (
        <Field
          id="oc-instance"
          label="Instance"
          hint="Each self-hosted instance has its own OAuth application. Leave on the default host for the SaaS application."
        >
          <select id="oc-instance" className={inputClass} value={instanceId} onChange={(e) => setInstanceId(e.target.value)}>
            <option value="">{selected.default_host ? `${selected.default_host} (default host)` : "Select…"}</option>
            {choices.map((i) => (
              <option key={i.id} value={i.id}>{`${i.label} (${i.host})`}</option>
            ))}
          </select>
        </Field>
      ) : null}
      {selected ? <DefinitionInputs def={selected.def} values={values} onChange={setValues} /> : null}
      {create.error ? <p role="alert" className="text-sm text-red-700">{errorMessage(create.error)}</p> : null}
      <div className="flex gap-2">
        <Button
          type="submit"
          size="sm"
          disabled={create.isPending || !selected || (!!selected.instance_bound && !selected.default_host && !instanceId)}
        >
          Add
        </Button>
        <Button type="button" size="sm" variant="outline" onClick={onDone}>Cancel</Button>
      </div>
    </form>
  );
}

function ClientRow({
  scope,
  c,
  canOwn,
  def,
  instanceHost,
}: {
  scope: CredScope;
  c: OAuthClient;
  canOwn: boolean;
  def?: OAuthClientDefinition;
  instanceHost?: string;
}) {
  const patch = usePatchOAuthClient(scope);
  const del = useDeleteOAuthClient(scope);
  const [mode, setMode] = useState<"none" | "edit" | "delete">("none");
  const [edits, setEdits] = useState<Record<string, string>>({});
  // A client only owns rows at its own scope (inherited ones are read-only here).
  const mine = c.scope === scope.kind;
  function startEdit() {
    // Non-secret values are prefilled from the API; secrets are write-only and start blank.
    const init: Record<string, string> = {};
    for (const f of def?.fields ?? []) init[f.name] = f.secret ? "" : (c.fields[f.name] ?? "");
    setEdits(init);
    setMode("edit");
  }
  async function save(e: FormEvent) {
    e.preventDefault();
    const changed: Record<string, string> = {};
    for (const [k, v] of Object.entries(edits)) if (v !== (c.fields[k] ?? "")) changed[k] = v;
    try {
      await patch.mutateAsync({ id: c.id, fields: buildClientPatchFields(changed) });
      setMode("none");
    } catch {
      // shown below
    }
  }
  return (
    <li className="space-y-2 rounded-md border border-neutral-200 p-3">
      <div className="flex flex-wrap items-baseline justify-between gap-2">
        <div>
          <span className="font-medium">{c.package_id}</span> <span className="text-xs text-neutral-500">{c.client_key} · {c.scope} · {c.connection_count} connection{c.connection_count === 1 ? "" : "s"}</span>
          {c.instance_id ? <span className="ml-2 font-mono text-xs text-muted">{instanceHost ?? "self-hosted instance"}</span> : null}
          {!c.enabled ? <span className="ml-2 rounded bg-neutral-100 px-1.5 py-0.5 text-xs">Disabled</span> : null}
        </div>
        {canOwn && mine ? (
          <div className="flex gap-1">
            <Button size="sm" variant="outline" disabled={patch.isPending} onClick={() => patch.mutate({ id: c.id, enabled: !c.enabled })}>{c.enabled ? "Disable" : "Enable"}</Button>
            <Button size="sm" variant="outline" onClick={startEdit}>Edit</Button>
            <Button size="sm" variant="outline" onClick={() => setMode("delete")} aria-label={`Delete ${c.client_key}`}>Delete</Button>
          </div>
        ) : null}
      </div>
      <dl className="text-xs text-neutral-600">
        {Object.entries(c.fields).map(([k, v]) => (
          <div key={k}><dt className="inline font-medium">{k}: </dt><dd className="inline">{v}</dd></div>
        ))}
      </dl>
      {mode === "edit" ? (
        <form onSubmit={(e) => void save(e)} aria-label={`Edit ${c.client_key}`} className="space-y-2">
          {def ? (
            <DefinitionInputs def={def} values={edits} onChange={setEdits} existing />
          ) : (
            <p className="text-sm text-neutral-600">No field definition is available for this client.</p>
          )}
          <p className="text-xs text-neutral-500">Secret values are never shown. Enter a new value to replace one.</p>
          {patch.error ? <p role="alert" className="text-sm text-red-700">{errorMessage(patch.error)}</p> : null}
          <div className="flex gap-2">
            <Button type="submit" size="sm" disabled={patch.isPending}>Save</Button>
            <Button type="button" size="sm" variant="outline" onClick={() => setMode("none")}>Cancel</Button>
          </div>
        </form>
      ) : null}
      {mode === "delete" ? (
        <div role="alertdialog" aria-label={`Delete ${c.client_key}`} className="space-y-2 text-sm">
          <p>Delete this OAuth client?</p>
          {del.error ? <p role="alert" className="text-red-700">{errorMessage(del.error)}</p> : null}
          <div className="flex gap-2">
            <Button size="sm" disabled={del.isPending} onClick={() => del.mutate(c.id, { onSuccess: () => setMode("none") })}>Confirm delete</Button>
            <Button size="sm" variant="outline" onClick={() => setMode("none")}>Cancel</Button>
          </div>
        </div>
      ) : null}
    </li>
  );
}

export function OAuthClientsPanel({ scope, canOwn, orgId }: { scope: CredScope; canOwn: boolean; orgId?: string }) {
  const list = useOAuthClients(scope);
  const defs = useDefinitions(scope);
  // Self-hosted instances a client can be bound to (an org scope also sees the instance-wide ones).
  const instScope: InstanceScope | undefined =
    scope.kind === "instance"
      ? { kind: "instance" }
      : scope.kind === "organization"
        ? { kind: "organization", id: scope.id }
        : orgId
          ? { kind: "organization", id: orgId }
          : undefined;
  const instances = useAppInstances(instScope).data?.items ?? [];
  const hosts = new Map(instances.map((i) => [i.id, i.host]));
  const [adding, setAdding] = useState(false);
  if (list.isPending) return <Loading />;
  if (list.isError) return <ErrorState error={list.error} onRetry={() => list.refetch()} />;
  return (
    <div className="space-y-3">
      {canOwn ? (
        adding ? (
          <CreateForm scope={scope} defs={defs} instances={instances} onDone={() => setAdding(false)} />
        ) : (
          <button type="button" className="text-sm underline" onClick={() => setAdding(true)}>Add an OAuth client</button>
        )
      ) : null}
      {list.data.items.length === 0 ? (
        <EmptyState title="No OAuth clients yet" />
      ) : (
        <ul className="space-y-2" aria-label="OAuth clients">
          {list.data.items.map((c) => (
            <ClientRow
              key={c.id}
              scope={scope}
              c={c}
              canOwn={canOwn}
              def={defs.get(`${c.package_id}|${c.client_key}`)?.def}
              instanceHost={c.instance_id ? hosts.get(c.instance_id) : undefined}
            />
          ))}
        </ul>
      )}
    </div>
  );
}
