"use client";

import { HowToCall } from "@/components/automations/webhook-howto";
import type { WebhookReveal } from "@/components/automations/webhook-form";
import { SecretBox, SecretOnce, useSecretCopy } from "@/components/ui/secret-once";

/** A second one-time value in the dialog (the secret beside the address), with its own Copy. */
function ValueBox({ label, value }: { label: string; value: string }) {
  const { used, state, copy } = useSecretCopy(value);
  return (
    <div className="space-y-1.5">
      <p className="text-sm font-medium text-ink">{label}</p>
      <SecretBox secret={value} used={used} state={state} onCopy={copy} />
    </div>
  );
}

/**
 * (P5-REFINE-14, webhooks-6, webhooks-1) The one-time dialog after a webhook is made or its address or secret renewed: the
 * address and the secret each in a box that fits the whole value on one line (the box scrolls sideways, it never wraps mid
 * token), Copy → Copied, the warning that closing loses them, and **How to call it** below. The values live in this
 * component's props only (the opener clears them on close).
 */
export function WebhookRevealDialog({ reveal, onClose }: { reveal: WebhookReveal | null; onClose: () => void }) {
  if (!reveal) return null;
  const { name, url, secret, kind, header, reason } = reveal;
  const main = url ?? secret ?? null;
  const title = reason === "secret" ? "Copy your new secret" : reason === "address" ? "Copy your new address" : "Copy your webhook address and secret";
  return (
    <SecretOnce
      secret={main}
      size="lg"
      title={title}
      label={url ? "Your address" : "Your secret"}
      description={`Webhook "${name}". Shown once: closing this loses it, so copy it to the sender now.`}
      onClose={onClose}
    >
      <div className="space-y-4">
        {url && secret ? <ValueBox label="Your secret" value={secret} /> : null}
        <HowToCall url={url} prefix={reveal.prefix} kind={kind} header={header} idBase="howto-reveal" />
      </div>
    </SecretOnce>
  );
}
