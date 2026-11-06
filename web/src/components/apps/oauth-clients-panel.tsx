"use client";

import { useState, type FormEvent } from "react";

import { EmptyState, ErrorState, Loading } from "@/components/states";
import { Button } from "@/components/ui/button";
import { Field, inputClass } from "@/components/ui/field";
import { errorMessage } from "@/lib/api/errors";
import {
  buildClientPatchFields,
  isSecretFieldName,
  useApps,
  useCreateOAuthClient,
  useDeleteOAuthClient,
  useOAuthClients,
  usePatchOAuthClient,
  type CredScope,
  type OAuthClient,
} from "@/lib/api/apps";
import { useQueries } from "@tanstack/react-query";
import { api } from "@/lib/api/client";
import { unwrap } from "@/lib/api/unwrap";

interface Row { name: string; value: string }

/** Names of the fields a client has (non-secret ones are returned by the API; secrets are write-only). */
function FieldInputs({ rows, onChange, existing }: { rows: Row[]; onChange: (r: Row[]) => void; existing?: boolean }) {
  return (
    <div className="space-y-2">
      {rows.map((r, i) => {
        const secret = isSecretFieldName(r.name);
        const id = `ocf-${i}`;
        return (
          <div key={i} className="flex gap-2">
            <input
              aria-label={`Field name ${i + 1}`}
              className={inputClass}
              value={r.name}
              readOnly={existing}
              onChange={(e) => onChange(rows.map((x, j) => (j === i ? { ...x, name: e.target.value } : x)))}
            />
            <input
              id={id}
              aria-label={`Value of ${r.name || `field ${i + 1}`}`}
              className={inputClass}
              type={secret ? "password" : "text"}
              autoComplete="off"
              placeholder={existing && secret ? "Leave blank to keep the stored value" : ""}
              value={r.value}
              onChange={(e) => onChange(rows.map((x, j) => (j === i ? { ...x, value: e.target.value } : x)))}
            />
            {!existing ? (
              <Button type="button" size="sm" variant="outline" aria-label={`Remove field ${i + 1}`} onClick={() => onChange(rows.filter((_, j) => j !== i))}>×</Button>
            ) : null}
          </div>
        );
      })}
      {!existing ? (
        <Button type="button" size="sm" variant="outline" onClick={() => onChange([...rows, { name: "", value: "" }])}>Add field</Button>
      ) : null}
    </div>
  );
}

function CreateForm({ scope, onDone }: { scope: CredScope; onDone: () => void }) {
  const create = useCreateOAuthClient(scope);
  const apps = useApps(scope.kind === "workspace" ? scope.id : undefined);
  const [choice, setChoice] = useState("");
  const [rows, setRows] = useState<Row[]>([{ name: "client_id", value: "" }, { name: "client_secret", value: "" }]);
  // Catalogue of (package, client_key) pairs; no app knowledge lives in the bundle.
  const detailQueries = useQueries({
    queries: (apps.data?.items ?? []).map((a) => ({
      queryKey: ["app", a.app_id, ""],
      queryFn: () => unwrap(api.GET("/v1/apps/{app}", { params: { path: { app: a.app_id } } })),
      retry: false,
    })),
  });
  const options = new Map<string, { package_id: string; client_key: string; label: string }>();
  for (const q of detailQueries)
    for (const m of q.data?.connect ?? [])
      if (m.client_key) options.set(`${q.data!.package_id}|${m.client_key}`, { package_id: q.data!.package_id, client_key: m.client_key, label: `${q.data!.name} (${m.client_key})` });
  async function submit(e: FormEvent) {
    e.preventDefault();
    const o = options.get(choice);
    if (!o) return;
    const fields: Record<string, string> = {};
    for (const r of rows) if (r.name.trim() && r.value !== "") fields[r.name.trim()] = r.value;
    try {
      await create.mutateAsync({ package_id: o.package_id, client_key: o.client_key, fields });
      setRows((rs) => rs.map((r) => ({ ...r, value: "" })));
      onDone();
    } catch {
      // shown below
    }
  }
  return (
    <form onSubmit={(e) => void submit(e)} className="space-y-3 rounded-md border border-neutral-200 p-3" aria-label="Add OAuth client">
      <Field id="oc-app" label="App">
        <select id="oc-app" className={inputClass} value={choice} onChange={(e) => setChoice(e.target.value)} required>
          <option value="">Select…</option>
          {[...options.entries()].map(([k, o]) => (
            <option key={k} value={k}>{o.label}</option>
          ))}
        </select>
      </Field>
      <FieldInputs rows={rows} onChange={setRows} />
      {create.error ? <p role="alert" className="text-sm text-red-700">{errorMessage(create.error)}</p> : null}
      <div className="flex gap-2">
        <Button type="submit" size="sm" disabled={create.isPending || !choice}>Add</Button>
        <Button type="button" size="sm" variant="outline" onClick={onDone}>Cancel</Button>
      </div>
    </form>
  );
}

function ClientRow({ scope, c, canOwn }: { scope: CredScope; c: OAuthClient; canOwn: boolean }) {
  const patch = usePatchOAuthClient(scope);
  const del = useDeleteOAuthClient(scope);
  const [mode, setMode] = useState<"none" | "edit" | "delete">("none");
  const [rows, setRows] = useState<Row[]>([]);
  // A client only owns rows at its own scope (inherited ones are read-only here).
  const mine = c.scope === scope.kind;
  function startEdit() {
    setRows(Object.keys(c.fields).map((k) => ({ name: k, value: c.fields[k] })));
    setMode("edit");
  }
  async function save(e: FormEvent) {
    e.preventDefault();
    const edits: Record<string, string> = {};
    for (const r of rows) if (r.value !== c.fields[r.name]) edits[r.name] = r.value;
    try {
      await patch.mutateAsync({ id: c.id, fields: buildClientPatchFields(edits) });
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
          <FieldInputs rows={rows} onChange={setRows} existing />
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

export function OAuthClientsPanel({ scope, canOwn }: { scope: CredScope; canOwn: boolean }) {
  const list = useOAuthClients(scope);
  const [adding, setAdding] = useState(false);
  if (list.isPending) return <Loading />;
  if (list.isError) return <ErrorState error={list.error} onRetry={() => list.refetch()} />;
  return (
    <div className="space-y-3">
      {canOwn ? (
        adding ? (
          <CreateForm scope={scope} onDone={() => setAdding(false)} />
        ) : (
          <button type="button" className="text-sm underline" onClick={() => setAdding(true)}>Add an OAuth client</button>
        )
      ) : null}
      {list.data.items.length === 0 ? (
        <EmptyState title="No OAuth clients yet" />
      ) : (
        <ul className="space-y-2" aria-label="OAuth clients">
          {list.data.items.map((c) => (
            <ClientRow key={c.id} scope={scope} c={c} canOwn={canOwn} />
          ))}
        </ul>
      )}
    </div>
  );
}
