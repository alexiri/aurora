"use client";

import Link from "next/link";

import { SecretOnce } from "@/components/ui/secret-once";
import { wsRoutes } from "@/lib/routes";

/** One new webhook's one-time secret, carried from the create response (C-1111: `secret` comes only there). */
export interface WebhookSecret {
  name: string;
  secret: string;
  /** The webhook's verification kind (`none` | `hmac_sha256` | `bearer`), said in the product's words in the dialog. */
  kind: string;
  enabled: boolean;
  agentName: string;
  handle: string;
}

/**
 * (C-1111, FIX-248) The one-time dialog for a webhook a template created: not a bare code, but what the code is (the
 * sender must present it with every alert so Aurora knows the alert is genuine and wakes the agent), why it must be
 * copied now (shown only this once), where to paste it (from the webhook's verification kind) and that the webhook
 * starts Paused. One-sentence lead, short list of steps; "Webhook 1 of 2" when the template made several.
 */
export function WebhookSecretDialog({
  webhook,
  index,
  count,
  org,
  ws,
  onClose,
  onOpenAutomations,
}: {
  webhook: WebhookSecret;
  /** Zero-based position among the template's new webhooks. */
  index: number;
  count: number;
  org: string;
  ws: string;
  onClose: () => void;
  /** Only offered on the last secret: following it ends the flow on the agent's Automations page. */
  onOpenAutomations?: () => void;
}) {
  const { name, secret, kind, enabled, agentName, handle } = webhook;
  const paste =
    kind === "bearer"
      ? "Paste it into the sender as the Bearer token of its contact point: the sender puts it in the Authorization header of every alert."
      : kind === "hmac_sha256"
        ? "Paste it into the sender as its signing secret: the sender signs every alert with it, and Aurora checks the signature header."
        : "The webhook checks nothing yet (Anyone with the URL). To check its senders, edit the webhook and choose Signed or Bearer token.";
  const automations = wsRoutes(org, ws).agent(handle, { tab: "automations" });
  return (
    <SecretOnce
      secret={secret}
      size="lg"
      title={`Secret for the webhook “${name}” of ${agentName}`}
      description={`The sender of this webhook must present this secret with every alert, so Aurora knows the alert is genuine and wakes ${agentName}.`}
      onClose={onClose}
    >
      <div className="space-y-3 text-sm text-ink-2">
        {count > 1 ? (
          <p className="text-xs font-medium text-muted" data-testid="webhook-secret-count">
            Webhook {index + 1} of {count}
          </p>
        ) : null}
        <ol className="list-decimal space-y-1.5 pl-5">
          <li>Copy it now: it is shown only this once and cannot be read again. If it is lost, make a new one by editing the webhook.</li>
          <li>{paste}</li>
          {!enabled ? (
            <li>
              The webhook starts Paused. Turn it on on{" "}
              {onOpenAutomations ? (
                <Link href={automations} className="underline" onClick={(e) => { e.preventDefault(); onOpenAutomations(); }}>
                  {agentName}&apos;s Automations page
                </Link>
              ) : (
                <>{agentName}&apos;s Automations page</>
              )}{" "}
              once the sender has the secret.
            </li>
          ) : null}
        </ol>
      </div>
    </SecretOnce>
  );
}
