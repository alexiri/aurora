"use client";

import Link from "next/link";
import { useState } from "react";

import { OwnerLabel, AnchorMenuItem } from "@/components/automations/owner-controls";
import { ReassignDialog, RowMenu, RowShell } from "@/components/automations/row-parts";
import { WebhookForm, type WebhookReveal } from "@/components/automations/webhook-form";
import { authKind, HowToCall } from "@/components/automations/webhook-howto";
import { ErrorState, Loading } from "@/components/states";
import { Altitude } from "@/components/ui/altitude";
import { Button } from "@/components/ui/button";
import { ConfirmDialog } from "@/components/ui/confirm-dialog";
import { CopyCurlButton } from "@/components/ui/copy-curl";
import { useOrigin } from "@/lib/use-origin";
import { webhookSnippet } from "@/lib/webhook-snippets";
import { LoadMore } from "@/components/ui/load-more";
import { MenuItem, MenuSeparator } from "@/components/ui/menu";
import { ErrorNotice, Notice } from "@/components/ui/notice";
import { ResponsiveTable } from "@/components/ui/responsive-table";
import { useUndoToast } from "@/components/ui/undo-toast";
import { When } from "@/components/ui/when";
import { useDeleteWebhook, useRotateWebhook, useRotateWebhookSecret, useUpdateWebhook, useWebhookDeliveries, type Webhook, type WebhookDelivery } from "@/lib/api/webhooks";
import { automationThreadHref } from "@/lib/automation-thread";
import { DELIVERY_RESULT, label } from "@/lib/labels";
import { personName } from "@/lib/workspace-name";

/** (webhooks-3) The deliveries as a table: When · Result · Size · Conversation; stacked rows below 900 px. */
export function Deliveries({ agentId, id, agentBase }: { agentId: string; id: string; agentBase: string }) {
  const q = useWebhookDeliveries(agentId, id, true);
  if (q.isPending) return <Loading />;
  if (q.isError) return <ErrorState error={q.error} onRetry={() => void q.refetch()} />;
  const items = q.data.pages.flatMap((p) => p.items);
  if (items.length === 0) return <p className="text-sm text-muted">No deliveries yet.</p>;
  return (
    <div className="flex flex-col gap-3">
      <ResponsiveTable<WebhookDelivery>
        caption="Deliveries"
        rows={items}
        rowKey={(d) => d.id}
        title={{ header: "When", cell: (d) => <When value={d.received_at} /> }}
        columns={[
          {
            key: "result",
            header: "Result",
            // A refusal is red text; a repeat is quiet; an accepted delivery is plain.
            cell: (d) => (
              <span className={d.result.startsWith("refused") ? "text-danger-ink" : d.result === "duplicate" ? "text-muted" : undefined}>{label(DELIVERY_RESULT, d.result)}</span>
            ),
          },
          { key: "size", header: "Size", cell: (d) => `${d.size} B` },
          {
            key: "conversation",
            header: "Conversation",
            cell: (d) => {
              const href = automationThreadHref(agentBase, d.conversation_id);
              return href ? (
                <Link href={href} className="text-accent-ink underline underline-offset-2 hover:no-underline">
                  Open the conversation
                </Link>
              ) : (
                "None"
              );
            },
          },
        ]}
      />
      <LoadMore query={q} />
    </div>
  );
}

export function WebhookRow({
  agentId,
  agentName,
  w,
  agentBase,
  wsId,
  canOwn,
  onReveal,
  select,
}: {
  select?: { checked: boolean; onChange: (on: boolean) => void };
  agentId: string;
  agentName: string;
  w: Webhook;
  agentBase: string;
  wsId?: string;
  canOwn?: boolean;
  onReveal: (r: WebhookReveal) => void;
}) {
  const origin = useOrigin();
  const [editing, setEditing] = useState(false);
  const [showDeliveries, setShowDeliveries] = useState(false);
  const [showHowTo, setShowHowTo] = useState(false);
  const [reassigning, setReassigning] = useState(false);
  const [confirm, setConfirm] = useState<"delete" | "rotate" | "secret" | null>(null);
  const rotate = useRotateWebhook(agentId);
  const rotateSecret = useRotateWebhookSecret(agentId);
  const del = useDeleteWebhook(agentId);
  const upd = useUpdateWebhook(agentId);
  const toast = useUndoToast();
  const kind = authKind(w.verification.kind);

  if (editing)
    return (
      <li className="p-4" data-automation-kind="webhook">
        <WebhookForm agentId={agentId} agentName={agentName} initial={w} embedded onDone={() => setEditing(false)} onReveal={onReveal} />
      </li>
    );

  const setEnabled = (enabled: boolean, announce: boolean) =>
    upd.mutate(
      { id: w.id, body: { enabled } },
      {
        onSuccess: () => {
          if (announce) toast(`${enabled ? "Resumed" : "Paused"} ${w.name}`, () => setEnabled(!enabled, false));
        },
      },
    );
  const err = rotate.error ?? rotateSecret.error ?? del.error ?? upd.error;

  return (
    <RowShell
      kind="webhook"
      name={w.name}
      on={w.enabled}
      select={select}
      when={w.enabled ? "When something arrives at its address" : "When something arrives at its address · paused, won't run until resumed"}
      body={w.instructions}
      meta={
        <>
          {w.paused_by ? `Paused by ${personName(w.paused_by.display_name)} · ` : ""}
          Last delivery: {w.last_delivery_at ? <When value={w.last_delivery_at} /> : "never"} · <OwnerLabel anchor={w.anchor} creator={w.created_by} />
        </>
      }
      notices={
        <>
          {w.disabled_reason === "authorization" ? (
            <Notice tone="pending" role="status">
              Paused because its owner lost access. Reassign it to a current member to bring it back.
            </Notice>
          ) : null}
          <ErrorNotice error={err} />
        </>
      }
      actions={
        <>
          <Button size="sm" variant="outline" aria-expanded={showHowTo} onClick={() => setShowHowTo((v) => !v)}>
            How to call it
          </Button>
          {/* (P5-REFINE-19, Expert) The same command the panel shows, for the address as the card has it; $SECRET, never the secret. */}
          <CopyCurlButton command={webhookSnippet("curl", { prefix: w.url_prefix, origin, kind, header: w.verification.header })} />
          {/* Below 640 px Deliveries stays on the row. */}
          <Button size="sm" variant="outline" className="sm:hidden" aria-expanded={showDeliveries} onClick={() => setShowDeliveries((v) => !v)}>
            Deliveries
          </Button>
          <RowMenu name={w.name}>
            <MenuItem onSelect={() => setEditing(true)}>Edit</MenuItem>
            <MenuItem onSelect={() => setShowHowTo((v) => !v)}>How to call it</MenuItem>
            <MenuItem className="max-sm:hidden" onSelect={() => setShowDeliveries((v) => !v)}>
              Deliveries
            </MenuItem>
            <MenuItem onSelect={() => setConfirm("rotate")}>Rotate URL…</MenuItem>
            {kind !== "none" ? <MenuItem onSelect={() => setConfirm("secret")}>New secret…</MenuItem> : null}
            <MenuItem disabled={upd.isPending} onSelect={() => setEnabled(!w.enabled, true)}>
              {w.enabled ? "Pause" : "Resume"}
            </MenuItem>
            {canOwn && wsId ? <MenuItem onSelect={() => setReassigning(true)}>Reassign…</MenuItem> : null}
            {canOwn ? <AnchorMenuItem name={w.name} anchor={w.anchor} busy={upd.isPending} onChange={(anchor) => upd.mutate({ id: w.id, body: { anchor } })} /> : null}
            <MenuSeparator />
            <MenuItem className="text-danger-ink" onSelect={() => setConfirm("delete")}>
              Delete…
            </MenuItem>
          </RowMenu>
        </>
      }
    >
      {showHowTo ? (
        <Altitude id="auto.howto" level="setup" promote>
          <HowToCall prefix={w.url_prefix} kind={kind} header={w.verification.header} addressShownOnce idBase={`howto-${w.id}`} />
        </Altitude>
      ) : null}
      {reassigning && wsId ? (
        <ReassignDialog open onOpenChange={setReassigning} agentId={agentId} wsId={wsId} kind="webhook" id={w.id} name={w.name} creatorId={w.created_by?.id} disabledReason={w.disabled_reason} />
      ) : null}
      {/* (P5-REFINE-2, webhooks-5) The second rung of the ladder: each says what stops working, and the verb is the button. */}
      <ConfirmDialog
        open={confirm === "rotate"}
        onOpenChange={(o) => !o && setConfirm(null)}
        title="Rotate the URL?"
        description="The old URL stops working now. Update every sender that uses it. The new URL is shown once."
        confirmLabel="Rotate URL"
        danger
        busy={rotate.isPending}
        onConfirm={() =>
          rotate.mutate(w.id, {
            onSuccess: (res) => {
              setConfirm(null);
              onReveal({ name: res.webhook.name, url: res.url, secret: res.secret ?? null, kind, header: w.verification.header, reason: "address" });
              rotate.reset();
            },
          })
        }
      />
      <ConfirmDialog
        open={confirm === "secret"}
        onOpenChange={(o) => !o && setConfirm(null)}
        title="Make a new secret?"
        description="Signatures made with the old secret are refused from now on. Update the sender with the new one first, or deliveries will fail."
        confirmLabel="Make new secret"
        danger
        busy={rotateSecret.isPending}
        onConfirm={() =>
          rotateSecret.mutate(w.id, {
            onSuccess: (res) => {
              setConfirm(null);
              onReveal({ name: res.webhook.name, secret: res.secret, kind, header: w.verification.header, prefix: w.url_prefix, reason: "secret" });
              rotateSecret.reset();
            },
          })
        }
      />
      <ConfirmDialog
        open={confirm === "delete"}
        onOpenChange={(o) => !o && setConfirm(null)}
        title={`Delete ${w.name}?`}
        description="The webhook and its delivery history are removed. Senders that call it get an error. Conversations it started are kept."
        confirmLabel="Delete webhook"
        danger
        busy={del.isPending}
        onConfirm={() => del.mutate(w.id, { onSuccess: () => setConfirm(null) })}
      />
      {showDeliveries ? <Deliveries agentId={agentId} id={w.id} agentBase={agentBase} /> : null}
    </RowShell>
  );
}
