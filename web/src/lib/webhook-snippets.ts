/**
 * (P5-REFINE-14, webhooks-1) "How to call it": the three snippets a sender needs, with the real address and the real
 * header name. The secret is never in a snippet: it reads `$SECRET` (curl) or the `WEBHOOK_SECRET` environment variable.
 */

export type WebhookAuth = "hmac_sha256" | "bearer" | "none";
export type SnippetLanguage = "curl" | "python" | "node";

export const SNIPPET_LANGUAGES: readonly { value: SnippetLanguage; label: string }[] = [
  { value: "curl", label: "curl" },
  { value: "python", label: "Python" },
  { value: "node", label: "Node" },
];

/** The header a signed webhook carries its signature in unless the webhook names another. */
export const DEFAULT_SIGNATURE_HEADER = "X-Signature";

export interface SnippetInput {
  /** The full address, or its prefix with `…` on a card (the full one was shown once). */
  url: string;
  kind: WebhookAuth;
  /** The header the webhook checks; `X-Signature` for signed, `Authorization` for bearer when absent. */
  header?: string | null;
}

/** One sentence under the panel's title, by kind. */
export function callSentence(kind: WebhookAuth, header?: string | null): string {
  if (kind === "hmac_sha256") {
    return `Send a POST with the body signed in the ${header || DEFAULT_SIGNATURE_HEADER} header: sha256= followed by the hex HMAC-SHA256 of the body.`;
  }
  if (kind === "bearer") {
    return `Send a POST with the header ${header || "Authorization"}: Bearer <token>.`;
  }
  return "Send a POST to the address.";
}

function curl({ url, kind, header }: SnippetInput): string {
  const body = `BODY='{"event":"deploy","status":"ok"}'`;
  if (kind === "hmac_sha256") {
    const h = header || DEFAULT_SIGNATURE_HEADER;
    return [
      body,
      `SIG=$(printf %s "$BODY" | openssl dgst -sha256 -hmac "$SECRET" | sed 's/^.* //')`,
      `curl -X POST ${url} -H "Content-Type: application/json" -H "${h}: sha256=$SIG" -d "$BODY"`,
    ].join("\n");
  }
  if (kind === "bearer") {
    const h = header || "Authorization";
    return [body, `curl -X POST ${url} -H "Content-Type: application/json" -H "${h}: Bearer $SECRET" -d "$BODY"`].join("\n");
  }
  return [body, `curl -X POST ${url} -H "Content-Type: application/json" -d "$BODY"`].join("\n");
}

function python({ url, kind, header }: SnippetInput): string {
  if (kind === "hmac_sha256") {
    const h = header || DEFAULT_SIGNATURE_HEADER;
    return [
      "import hashlib, hmac, json, os, urllib.request",
      "",
      `body = json.dumps({"event": "deploy", "status": "ok"}).encode()`,
      `sig = hmac.new(os.environ["WEBHOOK_SECRET"].encode(), body, hashlib.sha256).hexdigest()`,
      `req = urllib.request.Request("${url}", data=body, method="POST",`,
      `    headers={"Content-Type": "application/json", "${h}": "sha256=" + sig})`,
      "urllib.request.urlopen(req)",
    ].join("\n");
  }
  const auth =
    kind === "bearer"
      ? `"${header || "Authorization"}": "Bearer " + os.environ["WEBHOOK_SECRET"]`
      : null;
  return [
    "import json, os, urllib.request",
    "",
    `body = json.dumps({"event": "deploy", "status": "ok"}).encode()`,
    `req = urllib.request.Request("${url}", data=body, method="POST",`,
    `    headers={"Content-Type": "application/json"${auth ? `, ${auth}` : ""}})`,
    "urllib.request.urlopen(req)",
  ].join("\n");
}

function node({ url, kind, header }: SnippetInput): string {
  if (kind === "hmac_sha256") {
    const h = header || DEFAULT_SIGNATURE_HEADER;
    return [
      `import { createHmac } from "node:crypto";`,
      "",
      `const body = JSON.stringify({ event: "deploy", status: "ok" });`,
      `const sig = createHmac("sha256", process.env.WEBHOOK_SECRET).update(body).digest("hex");`,
      `await fetch("${url}", {`,
      `  method: "POST",`,
      `  headers: { "Content-Type": "application/json", "${h}": \`sha256=\${sig}\` },`,
      `  body,`,
      `});`,
    ].join("\n");
  }
  const auth = kind === "bearer" ? `, "${header || "Authorization"}": \`Bearer \${process.env.WEBHOOK_SECRET}\`` : "";
  return [
    `const body = JSON.stringify({ event: "deploy", status: "ok" });`,
    `await fetch("${url}", {`,
    `  method: "POST",`,
    `  headers: { "Content-Type": "application/json"${auth} },`,
    `  body,`,
    `});`,
  ].join("\n");
}

/** The snippet in one language. */
export function webhookSnippet(language: SnippetLanguage, input: SnippetInput): string {
  return language === "curl" ? curl(input) : language === "python" ? python(input) : node(input);
}
