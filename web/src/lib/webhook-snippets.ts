/**
 * (P5-REFINE-14, webhooks-1; FIX-207) "How to call it": the one place a webhook call is written (the card's panel, the
 * reveal dialog and the card's Copy as curl all come from here). The address is `"$WEBHOOK_URL"` (Python and Node: the
 * `WEBHOOK_URL` environment variable) with a comment naming where it comes from, unless the real address is known (right
 * after creating or rotating). The secret is never in a snippet: it reads `$SECRET` (curl) or the `WEBHOOK_SECRET`
 * environment variable.
 */

export type WebhookAuth = "hmac_sha256" | "bearer" | "none";
/** A stored verification kind (`none`, `hmac_sha256`, `bearer`) as the snippets know it. */
export function authKind(kind: string | null | undefined): WebhookAuth {
  return kind === "bearer" ? "bearer" : kind === "none" ? "none" : "hmac_sha256";
}

export type SnippetLanguage = "curl" | "python" | "node";

export const SNIPPET_LANGUAGES: readonly { value: SnippetLanguage; label: string }[] = [
  { value: "curl", label: "curl" },
  { value: "python", label: "Python" },
  { value: "node", label: "Node" },
];

/** The header a signed webhook carries its signature in unless the webhook names another. */
export const DEFAULT_SIGNATURE_HEADER = "X-Signature";

export interface SnippetInput {
  /** The full address, when it is known (right after creating or rotating); the snippets then use it as is. */
  url?: string | null;
  /** The token's display prefix (`auw_9xLPEkbX`), for the comment that says where `WEBHOOK_URL` comes from. */
  prefix?: string | null;
  /** The page's origin, for the same comment. */
  origin?: string;
  kind: WebhookAuth;
  /** The header the webhook checks; `X-Signature` for signed, `Authorization` for bearer when absent. */
  header?: string | null;
}

/** The address as each language writes it, and the comment line (with its own comment mark) above the command. */
function address(input: SnippetInput, lang: SnippetLanguage): { ref: string; comment: string[] } {
  if (input.url) {
    return { ref: `"${input.url}"`, comment: [] };
  }
  const mark = lang === "node" ? "//" : "#";
  const origin = (input.origin ?? "").replace(/\/+$/, "");
  const where = `${origin}/v1/hooks/${input.prefix || "auw_"}…`;
  const comment = [`${mark} WEBHOOK_URL is the address shown when you created it: ${where}`];
  return { ref: lang === "curl" ? '"$WEBHOOK_URL"' : lang === "python" ? 'os.environ["WEBHOOK_URL"]' : "process.env.WEBHOOK_URL", comment };
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

function curl(input: SnippetInput): string {
  const { kind, header } = input;
  const a = address(input, "curl");
  const url = a.ref;
  const body = `BODY='{"event":"deploy","status":"ok"}'`;
  if (kind === "hmac_sha256") {
    const h = header || DEFAULT_SIGNATURE_HEADER;
    return [
      ...a.comment,
      body,
      `SIG=$(printf %s "$BODY" | openssl dgst -sha256 -hmac "$SECRET" | sed 's/^.* //')`,
      `curl -X POST ${url} -H "Content-Type: application/json" -H "${h}: sha256=$SIG" -d "$BODY"`,
    ].join("\n");
  }
  if (kind === "bearer") {
    const h = header || "Authorization";
    return [...a.comment, body, `curl -X POST ${url} -H "Content-Type: application/json" -H "${h}: Bearer $SECRET" -d "$BODY"`].join("\n");
  }
  return [...a.comment, body, `curl -X POST ${url} -H "Content-Type: application/json" -d "$BODY"`].join("\n");
}

function python(input: SnippetInput): string {
  const { kind, header } = input;
  const a = address(input, "python");
  const url = a.ref;
  if (kind === "hmac_sha256") {
    const h = header || DEFAULT_SIGNATURE_HEADER;
    return [
      "import hashlib, hmac, json, os, urllib.request",
      "",
      ...a.comment,
      `body = json.dumps({"event": "deploy", "status": "ok"}).encode()`,
      `sig = hmac.new(os.environ["WEBHOOK_SECRET"].encode(), body, hashlib.sha256).hexdigest()`,
      `req = urllib.request.Request(${url}, data=body, method="POST",`,
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
    ...a.comment,
    `body = json.dumps({"event": "deploy", "status": "ok"}).encode()`,
    `req = urllib.request.Request(${url}, data=body, method="POST",`,
    `    headers={"Content-Type": "application/json"${auth ? `, ${auth}` : ""}})`,
    "urllib.request.urlopen(req)",
  ].join("\n");
}

function node(input: SnippetInput): string {
  const { kind, header } = input;
  const a = address(input, "node");
  const url = a.ref;
  if (kind === "hmac_sha256") {
    const h = header || DEFAULT_SIGNATURE_HEADER;
    return [
      `import { createHmac } from "node:crypto";`,
      "",
      ...a.comment,
      `const body = JSON.stringify({ event: "deploy", status: "ok" });`,
      `const sig = createHmac("sha256", process.env.WEBHOOK_SECRET).update(body).digest("hex");`,
      `await fetch(${url}, {`,
      `  method: "POST",`,
      `  headers: { "Content-Type": "application/json", "${h}": \`sha256=\${sig}\` },`,
      `  body,`,
      `});`,
    ].join("\n");
  }
  const auth = kind === "bearer" ? `, "${header || "Authorization"}": \`Bearer \${process.env.WEBHOOK_SECRET}\`` : "";
  return [
    ...a.comment,
    `const body = JSON.stringify({ event: "deploy", status: "ok" });`,
    `await fetch(${url}, {`,
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
