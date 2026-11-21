"use client";

import { useState, type FormEvent } from "react";

import { Altitude } from "@/components/ui/altitude";
import { Button } from "@/components/ui/button";
import { ChoiceCards } from "@/components/ui/choice-cards";
import { Field, inputClass } from "@/components/ui/field";
import { CheckboxField, FormActions } from "@/components/ui/form";
import { Notice, ErrorNotice } from "@/components/ui/notice";
import { SecretInput } from "@/components/ui/secret-input";
import { fieldErrors } from "@/lib/api/errors";
import { useCreateWebhook, useUpdateWebhook, type Webhook } from "@/lib/api/webhooks";
import { DEFAULT_SIGNATURE_HEADER, type WebhookAuth } from "@/lib/webhook-snippets";

import { authKind } from "./webhook-howto";

/** What the one-time dialog shows after a webhook is created or its address or secret was renewed. */
export interface WebhookReveal {
  name: string;
  /** The full address (create and rotate), shown once. */
  url?: string;
  /** The secret (made by the server, or the one the person gave), shown once. */
  secret?: string | null;
  kind: WebhookAuth;
  header?: string | null;
  /** The token's display prefix, when the full address is not known (a new secret): the snippets name it in a comment. */
  prefix?: string;
  /** Which dialog: a new webhook, a new address, or a new secret. */
  reason: "created" | "address" | "secret";
}

const ALPHABET = "ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz23456789";

/** A random 32-character secret from `crypto.getRandomValues` (no look-alike characters, so it reads cleanly aloud). */
export function generateSecret(length = 32): string {
  const out: string[] = [];
  const limit = 256 - (256 % ALPHABET.length);
  const buf = new Uint8Array(length * 2);
  while (out.length < length) {
    crypto.getRandomValues(buf);
    for (const b of buf) {
      if (out.length === length) break;
      if (b < limit) out.push(ALPHABET[b % ALPHABET.length]);
    }
  }
  return out.join("");
}

/**
 * (P5-REFINE-14, webhooks-2) The webhook form. **Who may call this webhook?** as three cards (Signed is the default and
 * recommended; Bearer token; Anyone with the URL last, with a note), a secret with **Generate**, and Advanced (the header,
 * and for owners keeping it running without them). Leaving the secret empty makes the server generate one (C-1078).
 * Rotating the secret is a menu item of the row, so Edit shows a secret field only when the kind changes.
 */
export function WebhookForm({
  agentId,
  agentName,
  initial,
  canOwn,
  embedded = false,
  onDone,
  onReveal,
}: {
  agentId: string;
  /** The agent's display name, as the people of the workspace see it. */
  agentName: string;
  initial?: Webhook;
  /** Workspace owners may anchor a new webhook to the workspace. */
  canOwn?: boolean;
  embedded?: boolean;
  onDone: () => void;
  onReveal: (reveal: WebhookReveal) => void;
}) {
  const create = useCreateWebhook(agentId);
  const update = useUpdateWebhook(agentId);
  const [name, setName] = useState(initial?.name ?? "");
  const [instructions, setInstructions] = useState(initial?.instructions ?? "");
  const [kind, setKind] = useState<WebhookAuth>(initial ? authKind(initial.verification.kind) : "hmac_sha256");
  const [header, setHeader] = useState(initial?.verification.header ?? "");
  const [secret, setSecret] = useState("");
  const [anchored, setAnchored] = useState(false);
  const [localErr, setLocalErr] = useState<string | null>(null);
  const mut = initial ? update : create;
  const fe = fieldErrors(mut.error);
  const kindChanged = !!initial && kind !== authKind(initial.verification.kind);
  const needsSecret = kind !== "none" && (!initial || kindChanged);
  const headerDefault = kind === "bearer" ? "Authorization" : DEFAULT_SIGNATURE_HEADER;

  function submit(e: FormEvent) {
    e.preventDefault();
    setLocalErr(null);
    const hdr = header.trim() || null;
    if (initial) {
      if (kindChanged && kind !== "none" && !secret) {
        setLocalErr("Add a secret, or press Generate.");
        return;
      }
      const headerChanged = hdr !== (initial.verification.header ?? null) && kind !== "none";
      const verification =
        kindChanged || secret !== "" || headerChanged ? { kind, header: kind === "none" ? null : hdr, secret: kind === "none" ? null : secret || undefined } : undefined;
      update.mutate(
        { id: initial.id, body: { name, instructions, verification } },
        { onSuccess: (w) => (kindChanged && kind !== "none" && secret ? (onReveal({ name: w.name, secret, kind, header: hdr, prefix: initial.url_prefix, reason: "secret" }), onDone()) : onDone()) },
      );
      return;
    }
    const verification =
      kind === "none"
        ? { kind: "none" as const }
        : secret
          ? { kind, header: hdr, secret }
          : { kind, header: hdr, generate_secret: true };
    create.mutate(
      { name, instructions, verification, ...(anchored ? { anchor: "workspace" } : {}) },
      {
        onSuccess: (res) => {
          onReveal({ name: res.webhook.name, url: res.url, secret: res.secret ?? (kind === "none" ? null : secret || null), kind, header: hdr, reason: "created" });
          onDone();
        },
      },
    );
  }

  return (
    <form
      onSubmit={submit}
      aria-label={initial ? "Edit webhook" : "New webhook"}
      className={embedded ? "space-y-4" : "space-y-4 rounded-card border border-line bg-surface p-4 shadow-card sm:p-5"}
    >
      <Field id="wh-name" label="Name" error={fe.name}>
        <input id="wh-name" className={inputClass} value={name} required placeholder="Deploy notice" onChange={(e) => setName(e.target.value)} />
      </Field>
      <Field id="wh-instr" label="Instructions" error={fe.instructions} hint={`Tells ${agentName} what to do with each delivery.`}>
        <textarea id="wh-instr" className={inputClass} rows={3} value={instructions} required onChange={(e) => setInstructions(e.target.value)} />
      </Field>

      {/* (P5-REFINE-3) Set up: a person at Use gets the recommended default and never has to choose. */}
      <Altitude id="auto.verification" level="setup">
        <div className="space-y-3">
          <ChoiceCards<WebhookAuth>
            legend="Who may call this webhook?"
            value={kind}
            onChange={setKind}
            options={[
              { value: "hmac_sha256", label: "Signed", badge: "Recommended", description: `Only senders that know the secret can wake ${agentName}.` },
              { value: "bearer", label: "Bearer token", description: "The sender puts a token in the Authorization header." },
              { value: "none", label: "Anyone with the URL", description: "No check. Use only for testing." },
            ]}
          />
          {kind === "none" ? (
            <Notice tone="pending" role="status">
              Anyone who has the address can wake {agentName}. Use this only for testing.
            </Notice>
          ) : null}
          {fe.verification ? (
            <p role="alert" className="text-xs text-danger-ink">
              {fe.verification}
            </p>
          ) : null}
        </div>
      </Altitude>

      {needsSecret ? (
        <Field
          id="wh-secret"
          label={kind === "bearer" ? "Bearer token" : "Signing secret"}
          error={fe["verification/secret"] ?? fe.secret ?? localErr ?? undefined}
          hint={initial ? "Generated for you, 32 characters." : "Generated for you, 32 characters. You'll see it once, so copy it to the sender. Leave it empty and Aurora makes one."}
        >
          <div className="flex flex-wrap gap-2">
            <div className="min-w-0 flex-1">
              <SecretInput id="wh-secret" revealLabel="secret" value={secret} onChange={setSecret} />
            </div>
            <Button type="button" variant="outline" onClick={() => setSecret(generateSecret())}>
              Generate
            </Button>
          </div>
        </Field>
      ) : null}

      {kind !== "none" || (!initial && canOwn) ? (
        <details className="rounded-control border border-line" open={!!header || undefined}>
          <summary className="min-h-10 cursor-pointer px-3 py-2 text-sm font-medium text-ink">Advanced</summary>
          <div className="space-y-4 border-t border-line p-3">
            {kind !== "none" ? (
              <Field id="wh-header" label="Header (optional)" hint={`The header that carries the ${kind === "bearer" ? "token" : "signature"}. Default: ${headerDefault}.`}>
                <input id="wh-header" className={inputClass} value={header} placeholder={headerDefault} onChange={(e) => setHeader(e.target.value)} />
              </Field>
            ) : null}
            {!initial && canOwn ? (
              <CheckboxField id="wh-anchored" label="Keep running without you" hint="The webhook keeps working even if you leave the workspace." checked={anchored} onChange={setAnchored} />
            ) : null}
          </div>
        </details>
      ) : null}

      {mut.error && Object.keys(fe).length === 0 ? <ErrorNotice error={mut.error} /> : null}
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
