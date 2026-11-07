"use client";

import { useState } from "react";

import { OneTimeKeyDialog } from "@/components/account/one-time-key-dialog";
import { MutationError } from "@/components/channels/shared";
import { StatusChip } from "@/components/channels/shared";
import { EmptyState, ErrorState, Loading } from "@/components/states";
import { Button } from "@/components/ui/button";
import { inputClass } from "@/components/ui/field";
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

/** Capabilities and setup fields announced by the pending manifest, for the reviewer. */
export function ManifestSummary({ manifest }: { manifest: unknown }) {
  const m = obj(manifest);
  const caps = Object.entries(obj(m.capabilities)).filter(([, v]) => v === true).map(([k]) => k);
  const integ = formFields(m.integration_setup);
  const pres = formFields(obj(obj(m.presence_setup).dedicated));
  return (
    <div className="space-y-2 text-sm" data-testid="manifest-summary">
      <div>
        <h4 className="font-medium">Capabilities</h4>
        {caps.length ? (
          <ul className="flex flex-wrap gap-1">
            {caps.map((c) => (
              <li key={c} className="rounded bg-neutral-100 px-1.5 py-0.5 font-mono text-xs">
                {c}
              </li>
            ))}
          </ul>
        ) : (
          <p className="text-neutral-500">None.</p>
        )}
      </div>
      <div>
        <h4 className="font-medium">Setup fields</h4>
        {integ.length + pres.length ? (
          <ul className="list-disc pl-5">
            {integ.map((f) => (
              <li key={`i-${f.name}`}>
                Integration: <code>{f.name}</code> ({f.type}, stored in {f.store})
              </li>
            ))}
            {pres.map((f) => (
              <li key={`p-${f.name}`}>
                Dedicated presence: <code>{f.name}</code> ({f.type}, stored in {f.store})
              </li>
            ))}
          </ul>
        ) : (
          <p className="text-neutral-500">None.</p>
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
    <li className="space-y-2 rounded-md border border-neutral-200 p-3">
      <div className="flex flex-wrap items-center gap-2">
        <h3 className="font-medium">{c.display_name}</h3>
        <code className="text-xs">{c.id}</code>
        <StatusChip status={c.status} />
        {c.version ? <span className="text-xs text-neutral-500">v{c.version}</span> : null}
        <span className="ml-auto text-xs text-neutral-500">
          {c.integration_count} integration(s) · {c.replicas.length} replica(s)
        </span>
      </div>
      {pending ? (
        <div className="space-y-2 rounded-md border border-amber-300 bg-amber-50 p-3 text-sm">
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
          <ManifestSummary manifest={c.manifest} />
          <label className="flex items-center gap-2">
            <input type="checkbox" checked={reviewed} onChange={(e) => setReviewed(e.target.checked)} />
            I have reviewed these capabilities and setup fields
          </label>
          <Button
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
        </div>
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
            <Button size="sm" disabled={del.isPending} onClick={() => del.mutate(c.id)}>
              Confirm delete
            </Button>
            <Button variant="outline" size="sm" onClick={() => setConfirmDel(false)}>
              Cancel
            </Button>
          </>
        ) : (
          <Button variant="outline" size="sm" onClick={() => setConfirmDel(true)}>
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
    <div className="max-w-3xl space-y-6">
      <h1 className="text-xl font-semibold">Connectors</h1>
      <form
        className="flex flex-wrap items-end gap-2"
        onSubmit={(e) => {
          e.preventDefault();
          provision.mutate(
            { id: id.trim(), display_name: name.trim() },
            {
              onSuccess: (r) => {
                setSecret({ id: r.connector.id, token: r.token });
                setId("");
                setName("");
              },
            },
          );
        }}
      >
        <div>
          <label htmlFor="conn-id" className="block text-sm font-medium">
            Connector id
          </label>
          <input
            id="conn-id"
            className={inputClass}
            pattern="[a-z][a-z0-9\-]{1,31}"
            required
            value={id}
            onChange={(e) => setId(e.target.value)}
          />
        </div>
        <div>
          <label htmlFor="conn-name" className="block text-sm font-medium">
            Display name
          </label>
          <input
            id="conn-name"
            className={inputClass}
            maxLength={80}
            required
            value={name}
            onChange={(e) => setName(e.target.value)}
          />
        </div>
        <Button type="submit" disabled={provision.isPending}>
          Provision
        </Button>
      </form>
      <MutationError error={provision.error} />

      {q.data.items.length === 0 ? (
        <EmptyState title="No connectors">Provision one to get a registration token.</EmptyState>
      ) : (
        <ul className="space-y-3">
          {q.data.items.map((c) => (
            <ConnectorCard key={c.id} c={c} onToken={(cid, token) => setSecret({ id: cid, token })} />
          ))}
        </ul>
      )}

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
