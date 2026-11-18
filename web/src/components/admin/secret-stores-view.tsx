"use client";

import { useState, type FormEvent } from "react";

import { EmptyState, ErrorState, Loading } from "@/components/states";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { ConfirmDialog, FormDialog } from "@/components/ui/confirm-dialog";
import { DataTable, Td, Th } from "@/components/ui/data-table";
import { Field, inputClass } from "@/components/ui/field";
import { Notice } from "@/components/ui/notice";
import { PageHeader } from "@/components/ui/page-header";
import { ItemList, ListRow, Section } from "@/components/ui/section";
import { StatusDot } from "@/components/ui/status-dot";
import { ApiError } from "@/lib/api/client";
import { errorMessage, fieldErrors } from "@/lib/api/errors";
import {
  useDeleteStore,
  usePutDefaultStore,
  usePutScopes,
  usePutStore,
  useSecretStores,
  useStartMigration,
  useTestStore,
  type MigrationView,
  type StoreInput,
  type StoreView,
  type StoresStatus,
  type TestResult,
} from "@/lib/api/secret-stores";

const STEP_LABEL: Record<string, string> = {
  configuration: "Definition is valid",
  authenticate: "Signed in to the store",
  write: "Wrote a probe value",
  read: "Read it back",
  delete: "Removed it",
};

const CODE_LABEL: Record<string, string> = {
  unreachable: "the store did not answer",
  invalid_config: "the definition is not valid",
  store_kind_not_allowed: "this kind of store cannot be added here",
  unexpected_value: "it returned an unexpected value",
  not_permitted: "the store refused it",
  not_found: "the value was not found",
  timeout: "it took too long",
  no_stored_secret: "there is no stored secret to keep",
  secret_reentry_required: "the address, role or CA changed, so enter the secret again",
  destination_refused: "this address is private, loopback or a metadata address; an operator can open it with gateway.allowed_private",
  internal: "internal error",
};

function when(iso: string | null | undefined): string {
  return iso ? new Date(iso).toLocaleString() : "";
}

/** The error of a write: stale, validation fields and everything else. */
function WriteError({ error, onReload }: { error: unknown; onReload: () => void }) {
  if (!error) return null;
  const code = error instanceof ApiError ? error.code : null;
  if (code === "stale_revision") {
    return (
      <Notice tone="pending" role="alert" className="space-y-2">
        <p>Someone changed the secret stores meanwhile. Reload and apply your change again.</p>
        <Button type="button" size="sm" variant="outline" onClick={onReload}>
          Reload
        </Button>
      </Notice>
    );
  }
  const fields = code === "settings_invalid" ? Object.entries(fieldErrors(error)) : [];
  return (
    <Notice tone="danger" role="alert">
      <p>{errorMessage(error)}</p>
      {fields.length > 0 ? (
        <ul className="mt-1 list-disc pl-5">
          {fields.map(([k, m]) => (
            <li key={k}>
              <code className="font-mono">{k}</code>: {m}
            </li>
          ))}
        </ul>
      ) : null}
    </Notice>
  );
}

// -------------------------------------------------------------------------- the store form

interface StoreDraft {
  id: string;
  address: string;
  kvMount: string;
  managedPrefix: string;
  /** Dashboard stores use AppRole only (D-307): the Kubernetes login is for Helm-declared stores. */
  method: "approle";
  authMount: string;
  roleId: string;
  secretId: string;
  caPem: string;
}

function str(v: unknown): string {
  return typeof v === "string" ? v : "";
}

function toDraft(s: StoreView | null): StoreDraft {
  const set = (s?.settings ?? {}) as Record<string, unknown>;
  const auth = (set.auth ?? {}) as Record<string, unknown>;
  return {
    id: s?.id ?? "",
    address: str(set.address),
    kvMount: str(set.kv_mount),
    managedPrefix: str(set.managed_prefix),
    method: "approle",
    authMount: str(auth.mount),
    roleId: str(auth.role_id),
    secretId: "",
    caPem: str(set.ca_pem),
  };
}

function hasSealedSecretId(s: StoreView | null): boolean {
  const auth = ((s?.settings ?? {}) as Record<string, unknown>).auth as Record<string, unknown> | undefined;
  return Boolean(auth?.secret_id);
}

function toInput(d: StoreDraft, existing: StoreView | null): StoreInput {
  const auth: Record<string, unknown> = { method: d.method };
  if (d.authMount.trim()) auth.mount = d.authMount.trim();
  auth.role_id = d.roleId.trim();
  auth.secret_id = { $secret: d.secretId.trim() || "keep" };
  const store: StoreInput = { kind: "hashicorp-vault", address: d.address.trim(), auth };
  if (d.kvMount.trim()) store.kv_mount = d.kvMount.trim();
  if (d.managedPrefix.trim()) store.managed_prefix = d.managedPrefix.trim();
  if (d.caPem.trim()) store.ca_pem = d.caPem.trim();
  if (existing && existing.reference_scopes.length > 0) {
    store.reference_scopes = existing.reference_scopes.map((r) => ({ org_id: r.org_id, paths: r.paths }));
  }
  return store;
}

function TestOutcome({ result }: { result: TestResult }) {
  return (
    <div role="status" aria-label="Connection test result" className="space-y-1 rounded-control border border-line p-3">
      <p className="text-sm font-medium">{result.ok ? "The connection works." : "The connection failed."}</p>
      <ul className="space-y-1">
        {result.steps.map((s) => (
          <li key={s.name}>
            <StatusDot tone={s.ok ? "accent" : "danger"}>
              {STEP_LABEL[s.name] ?? s.name}
              {s.error_code ? ` — ${CODE_LABEL[s.error_code.split(":")[0] ?? ""] ?? s.error_code}` : ""}
            </StatusDot>
          </li>
        ))}
      </ul>
    </div>
  );
}

function StoreDialog({
  existing,
  status,
  onClose,
}: {
  existing: StoreView | null;
  status: StoresStatus;
  onClose: () => void;
}) {
  const [d, setD] = useState<StoreDraft>(() => toDraft(existing));
  const [result, setResult] = useState<TestResult | null>(null);
  const put = usePutStore();
  const test = useTestStore();
  const sealed = hasSealedSecretId(existing);
  const set = <K extends keyof StoreDraft>(k: K, v: StoreDraft[K]) => {
    setResult(null);
    setD((x) => ({ ...x, [k]: v }));
  };
  function save(e?: FormEvent) {
    e?.preventDefault();
    put.mutate({ id: d.id.trim(), generation: status.generation, store: toInput(d, existing) }, { onSuccess: onClose });
  }
  function runTest() {
    setResult(null);
    test.mutate({ id: d.id.trim(), store: toInput(d, existing) }, { onSuccess: setResult });
  }
  const needsSecret = d.method === "approle" && !sealed && !d.secretId.trim();
  return (
    <FormDialog
      open
      onOpenChange={(o) => !o && onClose()}
      title={existing ? `Edit ${existing.id}` : "Add a Vault store"}
      description="A HashiCorp Vault KV v2 mount. Stored values are sealed and never shown again."
      submitLabel="Save store"
      busy={put.isPending}
      submitDisabled={needsSecret || !d.id.trim() || !d.address.trim()}
      onSubmit={save}
    >
      <Field id="ss-id" label="Store id" hint="Lowercase letters, digits, `_` or `-`; stored in every reference.">
        <input id="ss-id" required disabled={existing !== null} className={inputClass} value={d.id} onChange={(e) => set("id", e.target.value)} />
      </Field>
      <Field id="ss-address" label="Address" hint="https://vault.example.org:8200 (http only for loopback)">
        <input id="ss-address" required className={inputClass} value={d.address} onChange={(e) => set("address", e.target.value)} />
      </Field>
      <div className="grid gap-3 sm:grid-cols-2">
        <Field id="ss-kv" label="KV v2 mount" hint="Default: secret">
          <input id="ss-kv" className={inputClass} value={d.kvMount} onChange={(e) => set("kvMount", e.target.value)} />
        </Field>
        <Field id="ss-prefix" label="Managed prefix" hint="Where Aurora's own values live. Default: aurora">
          <input id="ss-prefix" className={inputClass} value={d.managedPrefix} onChange={(e) => set("managedPrefix", e.target.value)} />
        </Field>
      </div>
      <Field id="ss-method" label="Authentication" hint="AppRole. The Kubernetes login is available for stores declared in Helm.">
        <input id="ss-method" disabled className={inputClass} value="AppRole" readOnly />
      </Field>
      <Field id="ss-role-id" label="Role id">
        <input id="ss-role-id" required className={inputClass} value={d.roleId} onChange={(e) => set("roleId", e.target.value)} />
      </Field>
      <Field
        id="ss-secret-id"
        label="Secret id"
        hint={sealed ? "Set. Leave empty to keep it; type a new one to replace it." : "Sealed when saved; it is never shown again."}
      >
        <input
          id="ss-secret-id"
          type="password"
          autoComplete="new-password"
          className={inputClass}
          value={d.secretId}
          onChange={(e) => set("secretId", e.target.value)}
          placeholder={sealed ? "••••••••" : ""}
        />
      </Field>
      <Field id="ss-auth-mount" label="Auth mount" hint="Default: the method name">
        <input id="ss-auth-mount" className={inputClass} value={d.authMount} onChange={(e) => set("authMount", e.target.value)} />
      </Field>
      <Field id="ss-ca" label="CA certificate (PEM)" hint="Only for a private CA; it adds a trust anchor for this store.">
        <textarea id="ss-ca" rows={4} spellCheck={false} className={`${inputClass} font-mono`} value={d.caPem} onChange={(e) => set("caPem", e.target.value)} />
      </Field>
      <div className="flex flex-wrap items-center gap-3">
        <Button type="button" variant="outline" disabled={test.isPending || needsSecret || !d.id.trim() || !d.address.trim()} onClick={runTest}>
          {test.isPending ? "Testing…" : "Test connection"}
        </Button>
        <span className="text-xs text-muted">Signs in and writes, reads and removes a probe value. Nothing is saved.</span>
      </div>
      {test.isError ? (
        <Notice tone="danger" role="alert">
          {errorMessage(test.error)}
        </Notice>
      ) : null}
      {result ? <TestOutcome result={result} /> : null}
      <WriteError error={put.error} onReload={onClose} />
    </FormDialog>
  );
}

// ------------------------------------------------------------------------ reference scopes

interface ScopeRow {
  org: string;
  paths: string;
}

function ScopesDialog({ store, status, onClose }: { store: StoreView; status: StoresStatus; onClose: () => void }) {
  const [rows, setRows] = useState<ScopeRow[]>(() =>
    store.reference_scopes.map((r) => ({ org: r.org_id, paths: r.paths.join("\n") })),
  );
  const put = usePutScopes();
  function save(e: FormEvent) {
    e.preventDefault();
    const scopes = rows
      .filter((r) => r.org.trim())
      .map((r) => ({
        org_id: r.org.trim(),
        paths: r.paths
          .split("\n")
          .map((p) => p.trim())
          .filter(Boolean),
      }));
    put.mutate({ id: store.id, generation: status.generation, scopes }, { onSuccess: onClose });
  }
  return (
    <FormDialog
      open
      onOpenChange={(o) => !o && onClose()}
      title={`Reference scopes of ${store.id}`}
      description="Which organizations may reference which paths in this store. Paths are relative to the KV mount; a prefix ends in one `*`. The managed prefix is never allowed."
      submitLabel="Save scopes"
      busy={put.isPending}
      onSubmit={save}
    >
      {rows.length === 0 ? <p className="text-sm text-ink-2">No organization may reference this store yet.</p> : null}
      {rows.map((r, i) => (
        <div key={i} className="space-y-2 rounded-control border border-line p-3">
          <Field id={`sc-org-${i}`} label="Organization id">
            <input
              id={`sc-org-${i}`}
              className={`${inputClass} font-mono`}
              value={r.org}
              onChange={(e) => setRows((x) => x.map((y, j) => (j === i ? { ...y, org: e.target.value } : y)))}
            />
          </Field>
          <Field id={`sc-paths-${i}`} label="Paths (one per line)">
            <textarea
              id={`sc-paths-${i}`}
              rows={3}
              spellCheck={false}
              className={`${inputClass} font-mono`}
              value={r.paths}
              onChange={(e) => setRows((x) => x.map((y, j) => (j === i ? { ...y, paths: e.target.value } : y)))}
            />
          </Field>
          <Button type="button" size="sm" variant="outline" onClick={() => setRows((x) => x.filter((_, j) => j !== i))}>
            Remove organization
          </Button>
        </div>
      ))}
      <Button type="button" size="sm" variant="outline" onClick={() => setRows((x) => [...x, { org: "", paths: "" }])}>
        Add organization
      </Button>
      <WriteError error={put.error} onReload={onClose} />
    </FormDialog>
  );
}

// ----------------------------------------------------------------------------- the store list

function healthTone(s: StoreView): "accent" | "danger" {
  return s.health.ok ? "accent" : "danger";
}

function healthLabel(s: StoreView): string {
  if (s.health.ok) return "Healthy";
  return s.health.error_code === "init_failed" ? "Could not start" : "Not answering";
}

function SourceBadge({ s }: { s: StoreView }) {
  if (s.source === "helm") return <Badge tone="pending">Configured in Helm</Badge>;
  if (s.source === "builtin") return <Badge>Built in</Badge>;
  return <Badge tone="accent">Dashboard</Badge>;
}

function StoreRow({
  s,
  onEdit,
  onScopes,
  onRemove,
}: {
  s: StoreView;
  onEdit: () => void;
  onScopes: () => void;
  onRemove: () => void;
}) {
  const settings = s.settings as Record<string, unknown>;
  const auth = (settings.auth ?? {}) as Record<string, unknown>;
  const editable = s.source === "dashboard";
  return (
    <ListRow
      actions={
        editable ? (
          <>
            <Button size="sm" variant="outline" aria-label={`Edit ${s.id}`} onClick={onEdit}>
              Edit
            </Button>
            <Button size="sm" variant="outline" aria-label={`Reference scopes of ${s.id}`} onClick={onScopes}>
              Reference scopes
            </Button>
            <Button size="sm" variant="danger" aria-label={`Remove ${s.id}`} onClick={onRemove}>
              Remove
            </Button>
          </>
        ) : null
      }
    >
      <div className="min-w-0 space-y-1">
        <div className="flex flex-wrap items-center gap-2">
          <span className="font-medium">{s.id}</span>
          <Badge>{s.kind}</Badge>
          <SourceBadge s={s} />
          {s.default ? <Badge tone="accent">Default</Badge> : null}
          <StatusDot tone={healthTone(s)} title={s.health.checked_at ? `Checked ${when(s.health.checked_at)}` : undefined}>
            {healthLabel(s)}
          </StatusDot>
        </div>
        {s.kind === "hashicorp-vault" ? (
          <p className="break-all text-xs text-ink-2">
            {str(settings.address)} · mount {str(settings.kv_mount) || "secret"} · prefix {str(settings.managed_prefix) || "aurora"} ·{" "}
            {auth.method === "approle" ? "AppRole" : "Kubernetes"} auth
          </p>
        ) : null}
        {s.error ? <p className="text-xs text-danger-ink">{s.error}</p> : null}
        <p className="text-xs text-muted">
          {s.managed_values} managed value{s.managed_values === 1 ? "" : "s"}
          {s.references ? ` · ${s.reference_values} referenced` : ""}
        </p>
        {s.reference_scopes.length > 0 ? (
          <ul aria-label={`Reference scopes of ${s.id}`} className="text-xs text-ink-2">
            {s.reference_scopes.map((r) => (
              <li key={r.org_id} className="break-all">
                {r.org_slug ?? r.org_id}: <code className="font-mono">{r.paths.join(", ")}</code>
              </li>
            ))}
          </ul>
        ) : null}
        {s.source === "helm" ? (
          <p className="text-xs text-ink-2">
            Configured in Helm (<code className="font-mono">{s.helm_key ?? "vault.*"}</code>). Change it in the values file.
          </p>
        ) : null}
      </div>
    </ListRow>
  );
}

// ---------------------------------------------------------------------------- default store

function DefaultStore({ status, onReload }: { status: StoresStatus; onReload: () => void }) {
  const put = usePutDefaultStore();
  const [choice, setChoice] = useState(status.default_store.value);
  const locked = status.default_store.source === "helm";
  const usable = status.stores.filter((s) => s.health.ok && !s.error);
  return (
    <Section id="ss-default" title="Default store" description="Where new managed values go. Existing values stay where they are until you move them." card>
      {locked ? (
        <p className="text-sm">
          <code className="font-mono">{status.default_store.value}</code> — Configured in Helm (<code className="font-mono">vault.makeDefault</code>). Change it in the values file.
        </p>
      ) : (
        <form
          className="flex flex-wrap items-end gap-3"
          onSubmit={(e) => {
            e.preventDefault();
            put.mutate({ storeId: choice, generation: status.generation });
          }}
        >
          <Field id="ss-default-select" label="Store for new values">
            <select id="ss-default-select" className={inputClass} value={choice} onChange={(e) => setChoice(e.target.value)}>
              {usable.map((s) => (
                <option key={s.id} value={s.id}>
                  {s.id} ({s.kind})
                </option>
              ))}
            </select>
          </Field>
          <Button type="submit" disabled={put.isPending || choice === status.default_store.value}>
            Make default
          </Button>
        </form>
      )}
      {status.effective_default !== status.default_store.value ? (
        <Notice tone="danger" role="alert">
          The default store <code className="font-mono">{status.default_store.value}</code> could not start, so new values go to{" "}
          <code className="font-mono">{status.effective_default}</code> until it works again.
        </Notice>
      ) : null}
      <WriteError error={put.error} onReload={onReload} />
    </Section>
  );
}

// ---------------------------------------------------------------------------------- migration

function CountsTable({ m, label }: { m: MigrationView; label: string }) {
  if (m.per_purpose.length === 0) return <p className="text-sm text-ink-2">Nothing to move.</p>;
  return (
    <DataTable caption={label}>
      <thead>
        <tr>
          <Th>Kind of value</Th>
          <Th>{m.state === "dry_run" ? "Would move" : "Moved"}</Th>
          {m.state === "dry_run" ? null : <Th>Of</Th>}
          <Th>Already there</Th>
          <Th>Skipped</Th>
          <Th>Failed</Th>
        </tr>
      </thead>
      <tbody>
        {m.per_purpose.map((p) => (
          <tr key={p.purpose}>
            <Td>{p.purpose}</Td>
            <Td>{p.moved}</Td>
            {m.state === "dry_run" ? null : <Td>{p.total}</Td>}
            <Td>{p.already}</Td>
            <Td>{p.skipped}</Td>
            <Td>{p.failed}</Td>
          </tr>
        ))}
      </tbody>
    </DataTable>
  );
}

function Migration({ status, onReload }: { status: StoresStatus; onReload: () => void }) {
  const start = useStartMigration();
  const targets = status.stores.filter((s) => s.health.ok && !s.error);
  const [target, setTarget] = useState(targets.find((s) => s.default)?.id ?? targets[0]?.id ?? "inline");
  const [dry, setDry] = useState<MigrationView | null>(null);
  const [confirm, setConfirm] = useState(false);
  const m = status.migration;
  const running = m.state === "running" || m.running_elsewhere;
  function doStart() {
    start.mutate(
      { target, dryRun: false },
      {
        onSuccess: () => {
          setConfirm(false);
          setDry(null);
          onReload();
        },
        onError: () => setConfirm(false),
      },
    );
  }
  return (
    <Section
      id="ss-migrate"
      title="Move values"
      description="Moves every managed value to another store. It can be repeated: values already there are skipped, and an interrupted run picks up where it stopped."
      card
    >
      <div className="flex flex-wrap items-end gap-3">
        <Field id="ss-target" label="Move values to">
          <select id="ss-target" className={inputClass} value={target} onChange={(e) => { setTarget(e.target.value); setDry(null); }}>
            {targets.map((s) => (
              <option key={s.id} value={s.id}>
                {s.id}
              </option>
            ))}
          </select>
        </Field>
        <Button
          type="button"
          variant="outline"
          disabled={start.isPending || running}
          onClick={() => start.mutate({ target, dryRun: true }, { onSuccess: setDry })}
        >
          Preview what would move
        </Button>
        <Button type="button" disabled={start.isPending || running} onClick={() => setConfirm(true)}>
          {`Migrate to ${target}…`}
        </Button>
      </div>
      {running ? (
        <Notice tone="pending" role="status">
          {m.running_elsewhere && m.state !== "running"
            ? "A migration is running elsewhere (another replica or the command line). Progress is not visible here."
            : `Moving values to ${m.target ?? "the target"}…`}
        </Notice>
      ) : null}
      {dry ? <CountsTable m={dry} label={`What would move to ${dry.target ?? target}`} /> : null}
      {m.state === "running" ? <CountsTable m={m} label="Migration progress" /> : null}
      {m.state === "succeeded" || m.state === "failed" ? (
        <div className="space-y-2">
          <Notice tone={m.state === "succeeded" ? "accent" : "danger"} role="status">
            {m.state === "succeeded" ? "Last migration finished" : "Last migration did not finish"}
            {m.target ? ` (to ${m.target})` : ""}
            {m.finished_at ? ` ${when(m.finished_at)}` : ""}
            {m.started_by ? ` by ${m.started_by}` : ""}.{m.error ? ` ${m.error}` : ""}
          </Notice>
          <CountsTable m={m} label="Last migration report" />
        </div>
      ) : null}
      <WriteError error={start.error} onReload={onReload} />
      <ConfirmDialog
        open={confirm}
        onOpenChange={setConfirm}
        title={`Move every managed value to ${target}?`}
        description="Each value is copied to the new store and the old copy is deleted once the switch is committed. Organizations keep working while it runs."
        confirmLabel="Start migration"
        busy={start.isPending}
        onConfirm={doStart}
      />
    </Section>
  );
}

// ------------------------------------------------------------------------------------- the page

export function SecretStoresView() {
  const q = useSecretStores();
  const del = useDeleteStore();
  const [editing, setEditing] = useState<StoreView | "new" | null>(null);
  const [scopes, setScopes] = useState<StoreView | null>(null);
  const [removing, setRemoving] = useState<StoreView | null>(null);
  if (q.isPending) return <Loading />;
  if (q.isError) return <ErrorState error={q.error} onRetry={() => q.refetch()} />;
  const status = q.data;
  const reload = () => void q.refetch();
  return (
    <div className="max-w-page space-y-8">
      <PageHeader
        breadcrumb={[{ label: "Settings", href: "/admin/settings" }, { label: "Secret stores" }]}
        title="Secret stores"
        description="Where Aurora keeps secret values: the built-in store, or HashiCorp Vault. A store or key set in Helm is shown here read-only."
        actions={
          <Button onClick={() => setEditing("new")}>Add a Vault store</Button>
        }
      />
      <Section id="ss-stores" title="Stores">
        {status.stores.length === 0 ? (
          <EmptyState title="No stores" />
        ) : (
          <ItemList aria-label="Secret stores">
            {status.stores.map((s) => (
              <StoreRow key={s.id} s={s} onEdit={() => setEditing(s)} onScopes={() => setScopes(s)} onRemove={() => setRemoving(s)} />
            ))}
          </ItemList>
        )}
        <WriteError error={del.error} onReload={reload} />
      </Section>
      <DefaultStore key={`${status.generation}:${status.default_store.value}`} status={status} onReload={reload} />
      <Migration key={status.stores.length} status={status} onReload={reload} />
      {editing ? (
        <StoreDialog existing={editing === "new" ? null : editing} status={status} onClose={() => setEditing(null)} />
      ) : null}
      {scopes ? <ScopesDialog store={scopes} status={status} onClose={() => setScopes(null)} /> : null}
      <ConfirmDialog
        open={removing !== null}
        onOpenChange={(o) => !o && setRemoving(null)}
        title={`Remove ${removing?.id ?? "store"}`}
        description={
          removing && removing.managed_values + removing.reference_values > 0
            ? `It still holds ${removing.managed_values} managed and ${removing.reference_values} referenced value(s). Move them first; removal is refused until it is empty.`
            : "The store definition is removed. Nothing is deleted in the store itself."
        }
        confirmLabel="Remove store"
        danger
        busy={del.isPending}
        onConfirm={() =>
          removing &&
          del.mutate({ id: removing.id, generation: status.generation }, { onSettled: () => setRemoving(null) })
        }
      />
    </div>
  );
}
