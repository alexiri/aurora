"use client";

import { useState, type FormEvent } from "react";

import { OneTimeKeyDialog } from "@/components/account/one-time-key-dialog";
import { fmtDate } from "@/components/account/key-table";
import { EmptyState, ErrorState, Loading } from "@/components/states";
import { Button } from "@/components/ui/button";
import { Field, inputClass } from "@/components/ui/field";
import { AnchorSwitch, OwnerLabel, ReassignPanel } from "@/components/automations/owner-controls";
import { errorMessage, fieldErrors } from "@/lib/api/errors";
import {
  useCreateWebhook,
  useDeleteWebhook,
  useRotateWebhook,
  useUpdateWebhook,
  useWebhookDeliveries,
  useWebhooks,
  type Webhook,
} from "@/lib/api/webhooks";

const KINDS = ["none", "hmac_sha256", "bearer"] as const;

function Deliveries({ agentId, id }: { agentId: string; id: string }) {
  const q = useWebhookDeliveries(agentId, id, true);
  if (q.isPending) return <Loading />;
  if (q.isError) return <ErrorState error={q.error} onRetry={() => void q.refetch()} />;
  if (q.data.items.length === 0) return <p className="text-sm text-neutral-600">No deliveries yet.</p>;
  return (
    <table className="w-full text-left text-sm">
      <caption className="sr-only">Deliveries</caption>
      <tbody>
        {q.data.items.map((d) => (
          <tr key={d.id} className="border-t border-neutral-200">
            <td className="py-1 pr-4">{fmtDate(d.received_at)}</td>
            <td className="py-1 pr-4">{d.status}</td>
            <td className="py-1">{d.size} B</td>
          </tr>
        ))}
      </tbody>
    </table>
  );
}

export function WebhookForm({
  agentId,
  initial,
  canOwn,
  onDone,
  onSecret,
}: {
  agentId: string;
  initial?: Webhook;
  /** Workspace owners may anchor a new webhook to the workspace. */
  canOwn?: boolean;
  onDone: () => void;
  onSecret: (url: string, name: string) => void;
}) {
  const create = useCreateWebhook(agentId);
  const update = useUpdateWebhook(agentId);
  const [name, setName] = useState(initial?.name ?? "");
  const [instructions, setInstructions] = useState(initial?.instructions ?? "");
  const [kind, setKind] = useState<string>(initial?.verification.kind ?? "none");
  const [header, setHeader] = useState(initial?.verification.header ?? "");
  const [secret, setSecret] = useState("");
  const [anchored, setAnchored] = useState(false);
  const mut = initial ? update : create;
  const fe = fieldErrors(mut.error);

  function submit(e: FormEvent) {
    e.preventDefault();
    // Changing/keeping a kind other than "none" needs the secret re-sent; an unchanged verification is omitted on edit.
    const changed = !initial || kind !== initial.verification.kind || secret !== "" || (header || null) !== (initial.verification.header ?? null);
    const verification = changed ? { kind, header: header.trim() || null, secret: kind === "none" ? null : secret } : undefined;
    if (initial) {
      update.mutate({ id: initial.id, body: { name, instructions, verification } }, { onSuccess: onDone });
    } else {
      create.mutate(
        { name, instructions, verification, ...(anchored ? { anchor: "workspace" } : {}) },
        {
          onSuccess: (res) => {
            onSecret(res.url, res.webhook.name);
            onDone();
          },
        },
      );
    }
    setSecret("");
  }

  return (
    <form onSubmit={submit} aria-label={initial ? "Edit webhook" : "New webhook"} className="space-y-3 rounded-md border border-neutral-200 p-4">
      <Field id="wh-name" label="Name" error={fe.name}>
        <input id="wh-name" className={inputClass} value={name} required onChange={(e) => setName(e.target.value)} />
      </Field>
      <Field id="wh-instr" label="Instructions" error={fe.instructions} hint="Tells the agent what to do with each delivery.">
        <textarea id="wh-instr" className={inputClass} rows={3} value={instructions} required onChange={(e) => setInstructions(e.target.value)} />
      </Field>
      <Field id="wh-kind" label="Verification" error={fe.verification}>
        <select id="wh-kind" className={inputClass} value={kind} onChange={(e) => setKind(e.target.value)}>
          {KINDS.map((k) => (
            <option key={k} value={k}>
              {k}
            </option>
          ))}
        </select>
      </Field>
      {kind !== "none" ? (
        <div className="grid gap-3 sm:grid-cols-2">
          <Field id="wh-header" label="Header (optional)">
            <input id="wh-header" className={inputClass} value={header} onChange={(e) => setHeader(e.target.value)} />
          </Field>
          <Field id="wh-secret" label="Shared secret" hint={initial ? "Leave empty to keep the current secret." : undefined} error={fe["verification/secret"] ?? fe.secret}>
            <input id="wh-secret" type="password" autoComplete="off" className={inputClass} value={secret} onChange={(e) => setSecret(e.target.value)} />
          </Field>
        </div>
      ) : null}
      {!initial && canOwn ? (
        <label className="flex items-center gap-2 text-sm">
          <input type="checkbox" checked={anchored} onChange={(e) => setAnchored(e.target.checked)} />
          Anchor to workspace (outlives any single member)
        </label>
      ) : null}
      {mut.error && Object.keys(fe).length === 0 ? (
        <p role="alert" className="text-sm text-red-700">
          {errorMessage(mut.error)}
        </p>
      ) : null}
      <div className="flex gap-2">
        <Button type="submit" disabled={mut.isPending}>
          {initial ? "Save" : "Create webhook"}
        </Button>
        <Button type="button" variant="outline" onClick={onDone}>
          Cancel
        </Button>
      </div>
    </form>
  );
}

function WebhookRow({ agentId, w, onSecret, wsId, canOwn }: { agentId: string; w: Webhook; onSecret: (url: string, name: string) => void; wsId?: string; canOwn?: boolean }) {
  const [editing, setEditing] = useState(false);
  const [showDeliveries, setShowDeliveries] = useState(false);
  const [reassigning, setReassigning] = useState(false);
  const [confirm, setConfirm] = useState<"delete" | "rotate" | null>(null);
  const rotate = useRotateWebhook(agentId);
  const del = useDeleteWebhook(agentId);
  const upd = useUpdateWebhook(agentId);
  if (editing) return <WebhookForm agentId={agentId} initial={w} onDone={() => setEditing(false)} onSecret={onSecret} />;
  const err = rotate.error ?? del.error ?? upd.error;
  return (
    <li className="space-y-2 rounded-md border border-neutral-200 p-4">
      <div className="flex flex-wrap items-baseline gap-2">
        <h3 className="font-medium">{w.name}</h3>
        <code className="text-xs">{w.url_prefix}…</code>
        <span className="text-xs text-neutral-600">verification: {w.verification.kind}</span>
        <span className="ml-auto text-xs text-neutral-600">{w.enabled ? "Enabled" : `Disabled${w.disabled_reason ? ` (${w.disabled_reason})` : ""}`}</span>
      </div>
      <p className="whitespace-pre-wrap text-sm">{w.instructions}</p>
      <p className="text-xs text-neutral-600">
        Last delivery: {w.last_delivery_at ? fmtDate(w.last_delivery_at) : "never"} · <OwnerLabel anchor={w.anchor} creator={w.created_by} />
      </p>
      {w.disabled_reason === "authorization" ? (
        <p className="text-sm text-amber-800">Disabled because its owner lost access. Reassign it to a current member to bring it back.</p>
      ) : null}
      {canOwn ? <AnchorSwitch name={w.name} anchor={w.anchor} busy={upd.isPending} onChange={(anchor) => upd.mutate({ id: w.id, body: { anchor } })} /> : null}
      {reassigning && wsId ? (
        <ReassignPanel agentId={agentId} wsId={wsId} kind="webhook" id={w.id} name={w.name} creatorId={w.created_by?.id} disabledReason={w.disabled_reason} onClose={() => setReassigning(false)} />
      ) : null}
      {err ? (
        <p role="alert" className="text-sm text-red-700">
          {errorMessage(err)}
        </p>
      ) : null}
      <div className="flex flex-wrap gap-2">
        <Button size="sm" variant="outline" onClick={() => setEditing(true)}>
          Edit
        </Button>
        {canOwn && wsId ? (
          <Button size="sm" variant="outline" aria-expanded={reassigning} onClick={() => setReassigning((v) => !v)}>
            Reassign…
          </Button>
        ) : null}
        <Button size="sm" variant="outline" disabled={upd.isPending} onClick={() => upd.mutate({ id: w.id, body: { enabled: !w.enabled } })}>
          {w.enabled ? "Disable" : "Enable"}
        </Button>
        {confirm === "rotate" ? (
          <Button
            size="sm"
            disabled={rotate.isPending}
            onClick={() =>
              rotate.mutate(w.id, {
                onSuccess: (res) => {
                  setConfirm(null);
                  onSecret(res.url, res.webhook.name);
                  rotate.reset();
                },
              })
            }
          >
            Confirm rotate {w.name}
          </Button>
        ) : (
          <Button size="sm" variant="outline" onClick={() => setConfirm("rotate")}>
            Rotate URL
          </Button>
        )}
        <Button size="sm" variant="outline" aria-expanded={showDeliveries} onClick={() => setShowDeliveries((v) => !v)}>
          {showDeliveries ? "Hide deliveries" : "Deliveries"}
        </Button>
        {confirm === "delete" ? (
          <Button size="sm" disabled={del.isPending} onClick={() => del.mutate(w.id)}>
            Confirm delete {w.name}
          </Button>
        ) : (
          <Button size="sm" variant="outline" onClick={() => setConfirm("delete")}>
            Delete
          </Button>
        )}
      </div>
      {showDeliveries ? <Deliveries agentId={agentId} id={w.id} /> : null}
    </li>
  );
}

export function WebhooksTab({ agentId, wsId, canOwn }: { agentId: string; wsId?: string; canOwn?: boolean }) {
  const q = useWebhooks(agentId);
  const [creating, setCreating] = useState(false);
  // The URL lives only in this component state while the dialog is open.
  const [shown, setShown] = useState<{ url: string; name: string } | null>(null);
  if (q.isPending) return <Loading />;
  if (q.isError) return <ErrorState error={q.error} onRetry={() => void q.refetch()} />;
  return (
    <section aria-label="Webhooks" className="space-y-4">
      <div className="flex items-center">
        <h2 className="font-medium">Webhooks</h2>
        {!creating ? (
          <Button className="ml-auto" size="sm" onClick={() => setCreating(true)}>
            New webhook
          </Button>
        ) : null}
      </div>
      {creating ? <WebhookForm agentId={agentId} canOwn={canOwn} onDone={() => setCreating(false)} onSecret={(url, name) => setShown({ url, name })} /> : null}
      {q.data.items.length === 0 && !creating ? <EmptyState title="No webhooks">A webhook gives an external system a URL that wakes the agent.</EmptyState> : null}
      <ul className="space-y-2">
        {q.data.items.map((w) => (
          <WebhookRow key={w.id} agentId={agentId} w={w} wsId={wsId} canOwn={canOwn} onSecret={(url, name) => setShown({ url, name })} />
        ))}
      </ul>
      <OneTimeKeyDialog
        secret={shown?.url ?? null}
        name={shown?.name}
        title="Copy your webhook URL"
        description={`Webhook${shown ? ` "${shown.name}"` : ""} URL. You won't see this again, so store it somewhere safe now.`}
        onClose={() => setShown(null)}
      />
    </section>
  );
}
