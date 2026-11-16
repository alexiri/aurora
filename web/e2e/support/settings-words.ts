/**
 * The mocked settings sections' words (C-972): the label, help, choices and default of each key the Playwright mocks
 * serve, as the API's field registry writes them. A key without an entry gets a plain fixture label.
 */
type Json = Record<string, unknown>;

const CHOICES: Record<string, { value: string; label: string; description: string }[]> = {
  "governance.egress.minimum_posture": [
    { value: "open", label: "Any website", description: "Organizations decide where their agents may connect." },
    { value: "restricted", label: "Approved websites only", description: "Agents may only connect to websites on an approved list." },
    { value: "strict", label: "Only websites with an account", description: "Agents may only connect to websites an account was added for." },
  ],
  "byo.minimum_posture": [
    { value: "enabled", label: "Anyone in a workspace", description: "Any workspace member may connect their own agent." },
    { value: "owners_only", label: "Workspace owners only", description: "Only workspace owners may connect their own agent." },
    { value: "disabled", label: "Nobody", description: "Nobody may connect their own agent." },
  ],
  "auth.signup_mode": [
    { value: "invite_only", label: "Invited people only", description: "Organization owners and admins invite each person." },
    { value: "sso_jit", label: "Anyone with a verified e-mail", description: "They join at their first sign-in through your identity provider." },
    { value: "open", label: "Anyone", description: "Anyone who can reach this address may create an account." },
  ],
  "auth.org_creation": [
    { value: "instance_admins", label: "Aurora administrators only", description: "Only Aurora administrators create organizations." },
    { value: "anyone", label: "Anyone who is signed in", description: "Every person may create an organization of their own." },
  ],
};

const WORDS: Record<string, [label: string, help: string, kind: string, defaultDisplay: string, unit?: string]> = {
  "governance.egress.minimum_posture": ["Where agents may connect", "The strictest Internet access an organization may not go below.", "enum", "Any website"],
  "governance.llm.require_key_origin": ["Check where AI keys come from", "When on, a key only works for the provider it was added for.", "bool", "Off"],
  "governance.agent_tokens.max_age_days": ["Longest life of an agent token", "Lowering it shortens tokens that already exist.", "int", "No limit", "days"],
  "byo.default_source_cidrs": ["Addresses their agents may connect from", "Network ranges a new BYO agent accepts, one per line.", "list", "None"],
  "byo.minimum_posture": ["Who may connect their own agent", "The widest permission an organization may give.", "enum", "Workspace owners only"],
  "llm.enabled_providers": ["AI providers you offer", "The providers people may add AI keys for.", "list", "LiteLLM and Anthropic"],
  "llm.provider_preference": ["AI provider order", "The first provider in this list is used when more than one can answer.", "list", "LiteLLM, then Anthropic"],
  "llm.endpoints": ["AI model endpoints", "Servers your agents may send AI requests to.", "keyed_list", "None"],
  "auth.signup_mode": ["Who can sign up", "How a new person gets an account.", "enum", "Invited people only"],
  "auth.org_creation": ["Who can create an organization", "Who may start a new organization.", "enum", "Aurora administrators only"],
  "auth.session_idle_secs": ["Sign out after being away", "How long someone may be away before they sign in again.", "duration", "7 days", "days"],
  "auth.session_absolute_secs": ["Longest a sign-in lasts", "Everyone signs in again after this long.", "duration", "30 days", "days"],
  "auth.cli_key_ttl_days": ["How long a command-line key lasts", "0 means they never expire.", "int", "90 days", "days"],
  "auth.oidc_providers": ["How people sign in", "The identity providers people sign in with.", "keyed_list", "None"],
  "email.smtp_url": ["Mail server", "The address of the server that sends Aurora's e-mails.", "text", "Not set"],
  "email.from": ["Send as", "The address e-mails come from.", "text", "Not set"],
  "email.smtp_username": ["User name", "The account Aurora signs in with on the mail server.", "text", "Not set"],
  "email.smtp_password": ["Password", "Sent only to the mail server and never shown again once set.", "secret", "Not set"],
  "agents.default_name": ["Name of a new person's agent", "The name an agent gets unless its creator picks another.", "text", "Daneel"],
  "hosted.enabled_harnesses": ["Agent programs agents may run", "One per line; each must be offered by a runner.", "list", "claude-code"],
  "hosted.idle_stop_secs": ["Stop an idle computer after", "How long a computer may sit without activity.", "duration", "30 minutes", "minutes"],
  "hosted.turn_ceiling_secs": ["Longest one turn may run", "At least the longest a question may wait plus 15 minutes.", "duration", "6 hours", "hours"],
};

function fieldFor(s: Json): Json {
  const key = String(s.key);
  const w = WORDS[key];
  const schema = s.schema as { kind: string; options?: string[] | null };
  const kind = w?.[2] ?? (s.secret ? "secret" : s.list === "keyed" ? "keyed_list" : schema.kind === "duration_secs" ? "duration" : schema.kind === "string" ? "text" : schema.kind);
  return {
    key,
    label: w?.[0] ?? `Setting ${key}`,
    help: w?.[1] ?? `About ${key}.`,
    kind,
    choices: CHOICES[key] ?? null,
    default_display: w?.[3] ?? "Default",
    unit: w?.[4] ?? null,
    doc: null,
  };
}

/** A hand-built C-772 section as the API answers it today: with `fields`, editable, no `current`. */
export function withWords(body: unknown): unknown {
  if (typeof body !== "object" || body === null || !Array.isArray((body as Json).settings) || "fields" in body) return body;
  const b = body as Json;
  return { editable: true, current: null, ...b, fields: (b.settings as Json[]).map(fieldFor) };
}
