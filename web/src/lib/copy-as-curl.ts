/**
 * (P5-REFINE-19, D-406) The "Copy as curl" commands. They carry **placeholders for every secret** (`$SECRET`,
 * `$AURORA_API_KEY`) and never take a secret or a key value as an argument, so a command cannot contain one: the page
 * does not have them after they were shown once.
 */

import { authKind, webhookSnippet } from "@/lib/webhook-snippets";

/** Single-quote a value for a POSIX shell. */
export function shellQuote(value: string): string {
  return `'${value.replace(/'/g, `'\\''`)}'`;
}

/** An API call as the key's own row shows it: the base is the page's origin, the key is the environment variable. */
export function apiKeyCurl(origin: string): string {
  return `curl -H "Authorization: Bearer $AURORA_API_KEY" ${origin.replace(/\/+$/, "")}/v1/me`;
}

export interface WebhookCurlInput {
  /** The address to call: the real one when it is known (right after creating or rotating), else the stored prefix. */
  url: string;
  /** The full address is not known: the command says so in a comment above it. */
  prefixOnly?: boolean;
  /** `none` | `hmac_sha256` | `bearer`, as the API says. */
  kind: string;
  /** The effective header (`X-Signature`, `Authorization` by default); `null` for `none`. */
  header?: string | null;
}

/**
 * The command the "How to call it" panel shows for the webhook's verification, with `$SECRET` for the secret. It is built by
 * the panel's own function (`lib/webhook-snippets.ts`): one recipe source. On a card only the stored prefix is known, so the
 * command says so in a comment above it.
 */
export function webhookCurl({ url, prefixOnly = false, kind, header }: WebhookCurlInput): string {
  const note = prefixOnly ? "# The full address was shown once. Rotate the URL to get a new one, and put it here.\n" : "";
  return note + webhookSnippet("curl", { url: prefixOnly ? `${url}…` : url, kind: authKind(kind), header });
}
