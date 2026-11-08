"use client";

import { useState, type FormEvent } from "react";

import { EmptyState, ErrorState, Loading } from "@/components/states";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Field, inputClass } from "@/components/ui/field";
import { Form, FormActions } from "@/components/ui/form";
import { Notice } from "@/components/ui/notice";
import { errorMessage } from "@/lib/api/errors";
import {
  buildClientPatchFields,
  useApps,
  useCreateOAuthClient,
  useDeleteOAuthClient,
  useOAuthClients,
  usePatchOAuthClient,
  type CredScope,
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
      defs.set(`${q.data!.package_id}|${d.client_key}`, { package_id: q.data!.package_id, app_name: q.data!.name, def: d });
  return defs;
}

function RedirectUri({ uri }: { uri: string }) {
  const [copied, setCopied] = useState(false);
  return (
    <div className="flex flex-wrap items-center gap-2 text-xs text-ink-2">
      <span className="font-medium">Redirect URI:</span>
      <code className="break-all rounded-control border border-line bg-subtle px-1.5 py-0.5 font-mono text-ink">{uri}</code>
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
    <div className="space-y-3">
      {def.setup_note ? <p className="text-sm text-ink-2">{def.setup_note}</p> : null}
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

function CreateForm({ scope, onDone, defs }: { scope: CredScope; onDone: () => void; defs: Map<string, Definition> }) {
  const create = useCreateOAuthClient(scope);
  const [choice, setChoice] = useState("");
  const [values, setValues] = useState<Record<string, string>>({});
  const selected = defs.get(choice);
  async function submit(e: FormEvent) {
    e.preventDefault();
    if (!selected) return;
    const fields: Record<string, string> = {};
    for (const f of selected.def.fields) if ((values[f.name] ?? "") !== "") fields[f.name] = values[f.name];
    try {
      await create.mutateAsync({ package_id: selected.package_id, client_key: selected.def.client_key, fields });
      setValues({});
      onDone();
    } catch {
      // shown below
    }
  }
  return (
    <Form onSubmit={(e) => void submit(e)} className="rounded-card border border-line bg-surface p-4 shadow-card sm:p-5" aria-label="Add OAuth client">
      <Field id="oc-app" label="App">
        <select
          id="oc-app"
          className={inputClass}
          value={choice}
          onChange={(e) => {
            setChoice(e.target.value);
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
      {selected ? <DefinitionInputs def={selected.def} values={values} onChange={setValues} /> : null}
      {create.error ? <Notice tone="danger" role="alert">{errorMessage(create.error)}</Notice> : null}
      <FormActions>
        <Button type="submit" disabled={create.isPending || !selected}>Add</Button>
        <Button type="button" variant="outline" onClick={onDone}>Cancel</Button>
      </FormActions>
    </Form>
  );
}

function ClientRow({ scope, c, canOwn, def }: { scope: CredScope; c: OAuthClient; canOwn: boolean; def?: OAuthClientDefinition }) {
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
    <li className="space-y-3 rounded-card border border-line bg-surface p-4 shadow-card">
      <div className="flex flex-wrap items-center justify-between gap-x-3 gap-y-2">
        <div className="flex min-w-0 flex-wrap items-center gap-x-2 gap-y-1">
          <span className="font-medium text-ink">{c.package_id}</span>{" "}
          <span className="font-mono text-xs text-muted">{c.client_key} · {c.scope} · {c.connection_count} connection{c.connection_count === 1 ? "" : "s"}</span>
          {!c.enabled ? <Badge>Disabled</Badge> : null}
        </div>
        {canOwn && mine ? (
          <div className="flex flex-wrap gap-2">
            <Button size="sm" variant="outline" disabled={patch.isPending} onClick={() => patch.mutate({ id: c.id, enabled: !c.enabled })}>{c.enabled ? "Disable" : "Enable"}</Button>
            <Button size="sm" variant="outline" onClick={startEdit}>Edit</Button>
            <Button size="sm" variant="danger" onClick={() => setMode("delete")} aria-label={`Delete ${c.client_key}`}>Delete</Button>
          </div>
        ) : null}
      </div>
      <dl className="break-all text-xs text-ink-2">
        {Object.entries(c.fields).map(([k, v]) => (
          <div key={k}><dt className="inline font-medium">{k}: </dt><dd className="inline">{v}</dd></div>
        ))}
      </dl>
      {mode === "edit" ? (
        <Form onSubmit={(e) => void save(e)} aria-label={`Edit ${c.client_key}`} className="border-t border-line pt-3">
          {def ? (
            <DefinitionInputs def={def} values={edits} onChange={setEdits} existing />
          ) : (
            <p className="text-sm text-ink-2">No field definition is available for this client.</p>
          )}
          <p className="text-xs text-muted">Secret values are never shown. Enter a new value to replace one.</p>
          {patch.error ? <Notice tone="danger" role="alert">{errorMessage(patch.error)}</Notice> : null}
          <FormActions>
            <Button type="submit" size="sm" disabled={patch.isPending}>Save</Button>
            <Button type="button" size="sm" variant="outline" onClick={() => setMode("none")}>Cancel</Button>
          </FormActions>
        </Form>
      ) : null}
      {mode === "delete" ? (
        <Notice tone="danger" role="alertdialog" aria-label={`Delete ${c.client_key}`} className="space-y-2">
          <p>Delete this OAuth client?</p>
          {del.error ? <p role="alert">{errorMessage(del.error)}</p> : null}
          <div className="flex gap-2">
            <Button size="sm" variant="danger" disabled={del.isPending} onClick={() => del.mutate(c.id, { onSuccess: () => setMode("none") })}>Confirm delete</Button>
            <Button size="sm" variant="outline" onClick={() => setMode("none")}>Cancel</Button>
          </div>
        </Notice>
      ) : null}
    </li>
  );
}

export function OAuthClientsPanel({ scope, canOwn }: { scope: CredScope; canOwn: boolean }) {
  const list = useOAuthClients(scope);
  const defs = useDefinitions(scope);
  const [adding, setAdding] = useState(false);
  if (list.isPending) return <Loading />;
  if (list.isError) return <ErrorState error={list.error} onRetry={() => list.refetch()} />;
  return (
    <div className="space-y-3">
      {canOwn ? (
        adding ? (
          <CreateForm scope={scope} defs={defs} onDone={() => setAdding(false)} />
        ) : (
          <div>
            <Button variant="outline" onClick={() => setAdding(true)}>Add an OAuth client</Button>
          </div>
        )
      ) : null}
      {list.data.items.length === 0 ? (
        <EmptyState title="No OAuth clients yet" />
      ) : (
        <ul className="space-y-3" aria-label="OAuth clients">
          {list.data.items.map((c) => (
            <ClientRow key={c.id} scope={scope} c={c} canOwn={canOwn} def={defs.get(`${c.package_id}|${c.client_key}`)?.def} />
          ))}
        </ul>
      )}
    </div>
  );
}
