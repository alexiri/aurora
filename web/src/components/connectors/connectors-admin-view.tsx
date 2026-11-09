"use client";

import { useState } from "react";

import { OneTimeKeyDialog } from "@/components/account/one-time-key-dialog";
import { MutationError } from "@/components/channels/shared";
import { StatusChip } from "@/components/channels/shared";
import { EmptyState, ErrorState, Loading } from "@/components/states";
import { Button } from "@/components/ui/button";
import { Field, inputClass } from "@/components/ui/field";
import { Notice } from "@/components/ui/notice";
import { PageHeader } from "@/components/ui/page-header";
import { Section } from "@/components/ui/section";
import {
  formFields,
  useAdminConnectors,
  useApproveConnector,
  useDeleteConnector,
  useProvisionConnector,
  useRotateConnectorToken,
  type ConnectorAdmin,
} from "@/lib/api/connectors";

type Obj = Record<string, unknown>;
const obj = (v: unknown): Obj => (typeof v === "object" && v !== null ? (v as Obj) : {});

const enabledCaps = (m: Obj) =>
  new Map(Object.entries(obj(m.capabilities)).filter(([, v]) => v === true || (typeof v === "object" && v !== null)));

function keyedFields(m: Obj): Map<string, unknown> {
  const out = new Map<string, unknown>();
  for (const f of formFields(m.integration_setup)) out.set(`Integration: ${f.name}`, f);
  for (const f of formFields(obj(obj(m.presence_setup).dedicated))) out.set(`Dedicated presence: ${f.name}`, f);
  return out;
}

export type Change = { added: string[]; removed: string[]; changed: string[] };

function diffMaps(a: Map<string, unknown>, b: Map<string, unknown>): Change {
  const out: Change = { added: [], removed: [], changed: [] };
  for (const [k, v] of b) {
    if (!a.has(k)) out.added.push(k);
    else if (JSON.stringify(a.get(k)) !== JSON.stringify(v)) out.changed.push(k);
  }
  for (const k of a.keys()) if (!b.has(k)) out.removed.push(k);
  return out;
}

/** Old-vs-new manifest differences a reviewer must see (docs/14 manifest approval). */
export function diffManifest(approved: unknown, next: unknown) {
  const a = obj(approved);
  const b = obj(next);
  const cb = (m: Obj) => new Map((Array.isArray(m.public_callbacks) ? m.public_callbacks : []).map((c) => [String(c), c]));
  return {
    capabilities: diffMaps(enabledCaps(a), enabledCaps(b)),
    fields: diffMaps(keyedFields(a), keyedFields(b)),
    callbacks: diffMaps(cb(a), cb(b)),
  };
}

function ChangeList({ label, change }: { label: string; change: Change }) {
  const rows: [string, string, string[]][] = [
    ["Added", "text-danger-ink", change.added],
    ["Removed", "text-accent-ink", change.removed],
    ["Changed", "text-pending-ink", change.changed],
  ];
  const any = rows.some(([, , l]) => l.length);
  return (
    <div>
      <h5 className="font-medium text-ink">{label}</h5>
      {any ? (
        <ul className="list-disc pl-5">
          {rows.flatMap(([word, cls, list]) =>
            list.map((k) => (
              <li key={`${word}-${k}`} className={cls}>
                {word}: <code className="font-mono">{k}</code>
              </li>
            )),
          )}
        </ul>
      ) : (
        <p className="text-muted">No changes.</p>
      )}
    </div>
  );
}

/** Differences between the last approved manifest and the pending one. */
export function ManifestDiff({ approved, manifest }: { approved: unknown; manifest: unknown }) {
  const d = diffManifest(approved, manifest);
  return (
    <div className="space-y-2 text-sm" data-testid="manifest-diff">
      <h4 className="font-medium text-ink">Changes since the approved manifest</h4>
      <ChangeList label="Capabilities" change={d.capabilities} />
      <ChangeList label="Setup fields" change={d.fields} />
      <ChangeList label="Public callbacks" change={d.callbacks} />
    </div>
  );
}

/** Capabilities and setup fields announced by the pending manifest, for the reviewer. */
export function ManifestSummary({ manifest }: { manifest: unknown }) {
  const m = obj(manifest);
  const caps = Object.entries(obj(m.capabilities)).filter(([, v]) => v === true).map(([k]) => k);
  const integ = formFields(m.integration_setup);
  const pres = formFields(obj(obj(m.presence_setup).dedicated));
  return (
    <div className="space-y-2 text-sm" data-testid="manifest-summary">
      <div>
        <h4 className="font-medium text-ink">Capabilities</h4>
        {caps.length ? (
          <ul className="flex flex-wrap gap-1">
            {caps.map((c) => (
              <li key={c} className="rounded-full bg-subtle px-2 py-0.5 font-mono text-xs text-ink-2">
                {c}
              </li>
            ))}
          </ul>
        ) : (
          <p className="text-muted">None.</p>
        )}
      </div>
      <div>
        <h4 className="font-medium text-ink">Setup fields</h4>
        {integ.length + pres.length ? (
          <ul className="list-disc pl-5">
            {integ.map((f) => (
              <li key={`i-${f.name}`}>
                Integration: <code className="font-mono">{f.name}</code> ({f.type}, stored in {f.store})
              </li>
            ))}
            {pres.map((f) => (
              <li key={`p-${f.name}`}>
                Dedicated presence: <code className="font-mono">{f.name}</code> ({f.type}, stored in {f.store})
              </li>
            ))}
          </ul>
        ) : (
          <p className="text-muted">None.</p>
        )}
      </div>
    </div>
  );
}

function ConnectorCard({
  c,
  onToken,
}: {
  c: ConnectorAdmin;
  onToken: (id: string, token: string) => void;
}) {
  const approve = useApproveConnector();
  const rotate = useRotateConnectorToken();
  const del = useDeleteConnector();
  const [confirmDel, setConfirmDel] = useState(false);
  const [reviewed, setReviewed] = useState(false);
  const pending = c.status === "pending_approval";
  return (
    <li className="space-y-3 rounded-card border border-line bg-surface p-4 shadow-card sm:p-5">
      <div className="flex flex-wrap items-center gap-x-3 gap-y-1">
        <h3 className="font-display text-base font-semibold text-ink">{c.display_name}</h3>
        <code className="font-mono text-xs text-muted">{c.id}</code>
        <StatusChip status={c.status} />
        {c.version ? <span className="text-xs text-muted">v{c.version}</span> : null}
        <span className="text-xs text-muted sm:ml-auto">
          {c.integration_count} integration(s) · {c.replicas.length} replica(s)
        </span>
      </div>
      {pending ? (
        <Notice tone="pending" className="space-y-3">
          <h4 className="font-medium">Manifest awaiting approval</h4>
          {c.widening.length ? (
            <ul aria-label="Widening reasons" className="list-disc pl-5">
              {c.widening.map((w, i) => (
                <li key={i}>{w}</li>
              ))}
            </ul>
          ) : (
            <p>First registration.</p>
          )}
          {c.approved_manifest ? <ManifestDiff approved={c.approved_manifest} manifest={c.manifest} /> : null}
          <ManifestSummary manifest={c.manifest} />
          <label className="flex items-center gap-2 font-medium">
            <input type="checkbox" className="size-4 accent-accent" checked={reviewed} onChange={(e) => setReviewed(e.target.checked)} />
            I have reviewed these capabilities and setup fields
          </label>
          <Button
            variant="approve"
            disabled={!reviewed || !c.manifest_digest || approve.isPending}
            onClick={() => {
              approve.mutate(
                { id: c.id, manifest_digest: c.manifest_digest! },
                // A changed manifest must be reviewed again.
                { onError: () => setReviewed(false) },
              );
            }}
          >
            Approve manifest
          </Button>
        </Notice>
      ) : null}
      <div className="flex flex-wrap gap-2">
        <Button
          variant="outline"
          size="sm"
          disabled={rotate.isPending}
          onClick={() => rotate.mutate(c.id, {
              onSuccess: (r) => {
                onToken(c.id, r.token);
                rotate.reset();
              },
            })}
        >
          Rotate token
        </Button>
        {confirmDel ? (
          <>
            <Button variant="danger" size="sm" disabled={del.isPending} onClick={() => del.mutate(c.id)}>
              Confirm delete
            </Button>
            <Button variant="outline" size="sm" onClick={() => setConfirmDel(false)}>
              Cancel
            </Button>
          </>
        ) : (
          <Button variant="danger" size="sm" onClick={() => setConfirmDel(true)}>
            Delete
          </Button>
        )}
      </div>
      <MutationError error={approve.error ?? rotate.error ?? del.error} />
    </li>
  );
}

export function ConnectorsAdminView() {
  const q = useAdminConnectors();
  const provision = useProvisionConnector();
  const [id, setId] = useState("");
  const [name, setName] = useState("");
  const [secret, setSecret] = useState<{ id: string; token: string } | null>(null);

  if (q.isPending) return <Loading />;
  if (q.isError) return <ErrorState error={q.error} onRetry={() => q.refetch()} />;

  return (
    <div className="max-w-[1160px] space-y-8">
      <PageHeader title="Connectors" description="Chat-platform connectors that organizations can build integrations on." className="mb-6" />
      <Section id="conn-provision" title="Provision a connector" description="You get a registration token to give to the connector." card>
        <form
          className="flex flex-wrap items-start gap-x-4 gap-y-3"
          onSubmit={(e) => {
            e.preventDefault();
            provision.mutate(
              { id: id.trim(), display_name: name.trim() },
              {
                onSuccess: (r) => {
                  setSecret({ id: r.connector.id, token: r.token ?? "" });
                  setId("");
                  setName("");
                },
              },
            );
          }}
        >
          <Field id="conn-id" label="Connector id">
            <input
              id="conn-id"
              className={`${inputClass} w-56 max-w-full`}
              pattern="[a-z][a-z0-9\-]{1,31}"
              required
              value={id}
              onChange={(e) => setId(e.target.value)}
            />
          </Field>
          <Field id="conn-name" label="Display name">
            <input
              id="conn-name"
              className={`${inputClass} w-64 max-w-full`}
              maxLength={80}
              required
              value={name}
              onChange={(e) => setName(e.target.value)}
            />
          </Field>
          <Button type="submit" disabled={provision.isPending} className="sm:mt-[1.625rem]">
            Provision
          </Button>
        </form>
        <div className="mt-3 empty:hidden">
          <MutationError error={provision.error} />
        </div>
      </Section>

      <Section id="conn-list" title="Connectors">
        {q.data.items.length === 0 ? (
          <EmptyState title="No connectors">Provision one to get a registration token.</EmptyState>
        ) : (
          <ul className="space-y-4">
            {q.data.items.map((c) => (
              <ConnectorCard key={c.id} c={c} onToken={(cid, token) => setSecret({ id: cid, token })} />
            ))}
          </ul>
        )}
      </Section>

      <OneTimeKeyDialog
        secret={secret?.token ?? null}
        title="Copy the connector token"
        description={`Token for connector "${secret?.id ?? ""}". You won't see it again, so give it to the connector now.`}
        onClose={() => {
          setSecret(null);
          provision.reset();
        }}
      />
    </div>
  );
}
