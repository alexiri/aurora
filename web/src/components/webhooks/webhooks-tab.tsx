"use client";

import { useState, type FormEvent } from "react";

import { OneTimeKeyDialog } from "@/components/account/one-time-key-dialog";
import { fmtDate } from "@/components/account/key-table";
import { EmptyState, ErrorState, Loading } from "@/components/states";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { DataTable, Td } from "@/components/ui/data-table";
import { Field, inputClass } from "@/components/ui/field";
import { CheckboxField, FormActions } from "@/components/ui/form";
import { Notice } from "@/components/ui/notice";
import { ItemList, Section } from "@/components/ui/section";
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
  if (q.data.items.length === 0) return <p className="text-sm text-muted">No deliveries yet.</p>;
  return (
    <DataTable caption="Deliveries">
      <tbody>
        {q.data.items.map((d) => (
          <tr key={d.id}>
            <Td className="whitespace-nowrap">{fmtDate(d.received_at)}</Td>
            <Td>{d.status}</Td>
            <Td className="whitespace-nowrap">{d.size} B</Td>
          </tr>
        ))}
      </tbody>
    </DataTable>
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
    <form onSubmit={submit} aria-label={initial ? "Edit webhook" : "New webhook"} className="space-y-4 rounded-card border border-line bg-surface p-4 shadow-card sm:p-5">
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
        <div className="grid gap-4 sm:grid-cols-2">
          <Field id="wh-header" label="Header (optional)">
            <input id="wh-header" className={inputClass} value={header} onChange={(e) => setHeader(e.target.value)} />
          </Field>
          <Field id="wh-secret" label="Shared secret" hint={initial ? "Leave empty to keep the current secret." : undefined} error={fe["verification/secret"] ?? fe.secret}>
            <input id="wh-secret" type="password" autoComplete="off" className={inputClass} value={secret} onChange={(e) => setSecret(e.target.value)} />
          </Field>
        </div>
      ) : null}
      {!initial && canOwn ? (
        <CheckboxField id="wh-anchored" label="Anchor to workspace (outlives any single member)" checked={anchored} onChange={setAnchored} />
      ) : null}
      {mut.error && Object.keys(fe).length === 0 ? (
        <Notice tone="danger" role="alert">
          {errorMessage(mut.error)}
        </Notice>
      ) : null}
      <FormActions className="gap-2">
        <Button type="submit" disabled={mut.isPending}>
          {initial ? "Save" : "Create webhook"}
        </Button>
        <Button type="button" variant="outline" onClick={onDone}>
          Cancel
        </Button>
      </FormActions>
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
  if (editing)
    return (
      <li className="bg-canvas p-3 sm:p-4">
        <WebhookForm agentId={agentId} initial={w} onDone={() => setEditing(false)} onSecret={onSecret} />
      </li>
    );
  const err = rotate.error ?? del.error ?? upd.error;
  return (
    <li className="space-y-3 p-4">
      <div className="flex flex-wrap items-center gap-x-3 gap-y-1">
        <h3 className="font-display text-base font-semibold text-ink">{w.name}</h3>
        <code className="break-all font-mono text-xs text-ink-2">{w.url_prefix}…</code>
        <span className="text-xs text-muted">verification: {w.verification.kind}</span>
        <Badge className="sm:ml-auto" tone={w.enabled ? "accent" : "neutral"}>
          {w.enabled ? "Enabled" : `Disabled${w.disabled_reason ? ` (${w.disabled_reason})` : ""}`}
        </Badge>
      </div>
      <p className="whitespace-pre-wrap text-sm text-ink-2">{w.instructions}</p>
      <p className="text-xs text-muted">
        Last delivery: {w.last_delivery_at ? fmtDate(w.last_delivery_at) : "never"} · <OwnerLabel anchor={w.anchor} creator={w.created_by} />
      </p>
      {w.disabled_reason === "authorization" ? (
        <Notice tone="pending" role="status">
          Disabled because its owner lost access. Reassign it to a current member to bring it back.
        </Notice>
      ) : null}
      {canOwn ? <AnchorSwitch name={w.name} anchor={w.anchor} busy={upd.isPending} onChange={(anchor) => upd.mutate({ id: w.id, body: { anchor } })} /> : null}
      {reassigning && wsId ? (
        <ReassignPanel agentId={agentId} wsId={wsId} kind="webhook" id={w.id} name={w.name} creatorId={w.created_by?.id} disabledReason={w.disabled_reason} onClose={() => setReassigning(false)} />
      ) : null}
      {err ? (
        <Notice tone="danger" role="alert">
          {errorMessage(err)}
        </Notice>
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
            variant="danger"
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
          <Button size="sm" variant="danger" disabled={del.isPending} onClick={() => del.mutate(w.id)}>
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
    <Section
      id="webhooks"
      title="Webhooks"
      description="Give an external system a URL that wakes the agent."
      actions={
        !creating ? (
          <Button size="sm" onClick={() => setCreating(true)}>
            New webhook
          </Button>
        ) : null
      }
    >
      {creating ? <WebhookForm agentId={agentId} canOwn={canOwn} onDone={() => setCreating(false)} onSecret={(url, name) => setShown({ url, name })} /> : null}
      {q.data.items.length === 0 && !creating ? <EmptyState title="No webhooks">A webhook gives an external system a URL that wakes the agent.</EmptyState> : null}
      {q.data.items.length > 0 ? (
        <ItemList aria-label="Webhook list">
          {q.data.items.map((w) => (
            <WebhookRow key={w.id} agentId={agentId} w={w} wsId={wsId} canOwn={canOwn} onSecret={(url, name) => setShown({ url, name })} />
          ))}
        </ItemList>
      ) : null}
      <OneTimeKeyDialog
        secret={shown?.url ?? null}
        name={shown?.name}
        title="Copy your webhook URL"
        description={`Webhook${shown ? ` "${shown.name}"` : ""} URL. You won't see this again, so store it somewhere safe now.`}
        onClose={() => setShown(null)}
      />
    </Section>
  );
}
