/**
 * (P5-REFINE-19, D-406) The "Copy as curl" commands. They carry **placeholders for every secret** (`$SECRET`,
 * `$AURORA_API_KEY`) and never take a secret or a key value as an argument, so a command cannot contain one: the page
 * does not have them after they were shown once. A webhook's command is `webhookSnippet("curl", …)` (FIX-207): one generator.
 */

/** Single-quote a value for a POSIX shell. */
export function shellQuote(value: string): string {
  return `'${value.replace(/'/g, `'\\''`)}'`;
}

/** An API call as the key's own row shows it: the base is the page's origin, the key is the environment variable. */
export function apiKeyCurl(origin: string): string {
  return `curl -H "Authorization: Bearer $AURORA_API_KEY" ${origin.replace(/\/+$/, "")}/v1/me`;
}

