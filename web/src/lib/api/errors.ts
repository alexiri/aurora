import { ApiError } from "./client";

/** Friendly messages for M1 error codes (`docs/API_CONTRACTS.md`). */
export const ERROR_MESSAGES: Record<string, string> = {
  invalid_request: "Some of the values are not valid. Please check the form.",
  unauthorized: "Your session has expired. Please sign in again.",
  account_disabled: "This account has been disabled.",
  user_disabled: "This account has been disabled.",
  signup_closed:
    "Sign-up is closed on this instance. Ask an administrator for an invitation, then sign in again.",
  instance_setup_pending: "This instance is being set up — try again shortly.",
  email_taken: "An account with this email already exists.",
  rate_limited: "Too many requests. Please wait a moment and try again.",
  too_many_streams: "Too many open tabs or streams. Close some and reload.",
  csrf_rejected: "The request was rejected for security reasons. Reload the page and try again.",
  forbidden: "You do not have permission to do that.",
  not_found: "We could not find what you were looking for.",
  slug_taken: "That URL name is already in use. Choose another one.",
  org_creation_forbidden:
    "Only an instance administrator can create organizations on this instance. Ask an administrator to create one for you.",
  confirmation_mismatch: "The confirmation text does not match.",
  last_owner: "This would leave the organization without an owner.",
  member_limit_reached: "The member limit for this organization has been reached.",
  already_member: "This person is already a member.",
  not_an_org_member: "The user must be a member of the organization first.",
  implicit_access: "Organization owners and admins always have access.",
  invitation_expired: "This invitation has expired.",
  email_mismatch: "This invitation was sent to a different email address.",
  email_unverified:
    "Your sign-in provider has not verified this email address, so Aurora cannot trust it. Verify it with the provider and sign in again.",
  email_linked_elsewhere:
    "An account with this email address already exists through another sign-in provider, and this provider is not allowed to take it over. Sign in with the provider you used before.",
  invalid_token: "This link is invalid, has already been used, or has expired.",
  reauth_required: "Please confirm it is you by signing in again with your provider.",
  reauth_user_mismatch: "You signed in as a different user. Sign in again as the account you are using.",
  reauth_stale: "Your sign-in provider did not confirm a fresh sign-in, so nothing was changed. Please try again. If it keeps happening, an administrator can find the details in the audit log (sign-in denied, reason reauth_stale).",
  authorization_pending: "Waiting for approval.",
  expired_token: "This code has expired. Start again from your terminal.",
  access_denied: "The request was denied.",
  oidc_state_invalid: "The single sign-on attempt expired or was invalid. Please try again.",
  oidc_failed: "Single sign-on failed. Please try again or use another method.",
  email_domain_not_allowed: "Your email domain is not allowed to sign in with this provider.",
  cannot_modify_self: "You cannot change your own administrator status or disable your own account.",
  handle_taken: "That handle is already used by another agent in this workspace.",
  hosted_disabled: "Hosted agents are not enabled on this instance.",
  agent_limit_reached: "The agent limit for this organization has been reached.",
  hosted_agent_token_hidden: "Hosted agents do not expose a token.",
  gateway_ca_unavailable:
    "The gateway CA certificate is not available. Ask your operator to enable the gateway and its CA.",
  llm_key_not_permitted: "Your organization's governance does not permit this key here (endpoint or scope).",
  llm_key_wrong_organization:
    "This key belongs to a different organization account than the one governance requires.",
  llm_key_unverifiable: "We could not verify which account this key belongs to, and governance requires it.",
  llm_endpoint_unreachable: "The LLM endpoint could not be reached. Try again later.",
  llm_key_invalid: "The provider rejected this key as invalid.",
  llm_host_requires_llm_key: "This host is an LLM endpoint; create an LLM key instead of a generic secret.",
  reference_not_permitted: "Referencing an external secret store is not permitted here.",
  secret_store_unavailable: "The secret store is unavailable. Try again later.",
  name_taken: "That name is already in use.",
  app_not_instance_bound: "This app does not support self-hosted instances.",
  instance_host_invalid:
    "Enter the host name only, in lower case, for example gitlab.example.com: no https://, port, path, user name or IP address.",
  // Generic fallback; the holder-specific text is `hostConflictMessage` (FIX-132, `details.conflict`).
  instance_host_conflict:
    "That host is already claimed by another app, an LLM endpoint or another registered self-hosted app.",
  instance_already_wide: "This self-hosted app is already available to every organization.",
  instance_host_shared:
    "Another registration of this host exists. Delete it first, then make this one available to every organization.",
  // Fallback; the specific text is `probeFailedMessage` (FIX-144, `details.reason`).
  instance_probe_failed: "That host could not be reached from the Aurora server.",
  instance_base_path_invalid:
    "The path must start with / and have at most 8 segments of letters, digits and . _ ~ - (for example /jira), 128 characters in all.",
  instance_required: "Choose which instance to connect to.",
  instance_oauth_client_required:
    "Ask an admin to register a sign-in app for this instance first (Apps & accounts, Sign-in apps).",
  instance_deleted: "The self-hosted app of this connection was deleted. Connect again.",
  not_proposed: "This secret is no longer awaiting a value.",
  stale_revision: "The policy was changed by someone else. Reload to see the latest version.",
  policy_invalid: "The policy has errors. Fix the highlighted rules and try again.",
  governance_invalid: "The governance settings have errors. Fix the highlighted fields and try again.",
  governance_below_instance_minimum: "These settings are below the minimum required by the instance.",
  schedule_busy: "This schedule already has a run in progress. Try again in a moment.",
  already_decided: "Someone else already decided this.",
  too_many_attachments: "Too many attachments on one message. Remove some and try again.",
  payload_too_large: "That file is too large.",
  instance_admin_required: "This requires an instance administrator.",
  // Git skill library (phase 4, P4-GSL). Codes that carry a reason from the git host
  // (`library_unreachable`, `library_unavailable`, `library_skill_invalid`, `library_budget_exceeded`)
  // are shown with the server's message.
  library_exists: "A library with this label already exists in this organization.",
  library_limit: "This organization has reached its limit of connected libraries.",
  library_check_too_soon: "This library was checked a moment ago. Try again in a minute.",
  library_paused: "Resume the library before checking it.",
  library_skill_readonly: "This skill is managed by a skill library. Detach it to edit it.",
  update_superseded: "This update changed or was decided in the meantime. Reload and review it again.",
  connection_not_org_scoped: "A skill library needs an organization-level connection, not a workspace one.",
  app_without_library: "This connection's app cannot serve a skill library.",
  commit_required: "Reload the library list so the page knows which commit you are adding.",
  library_tree_too_large: "This repository is too large to list. Connect a narrower folder.",
  skill_libraries_disabled: "Skill libraries are turned off on this instance.",
  // (P5-UX-1, C-915, C-958, C-980) Refusals the connect journey and the access choices show in words.
  connect_probe_failed: "The app did not accept these details. Check them and try again.",
  oauth_client_required: "Signing in to this app needs a sign-in app that an admin has to register first.",
  above_account_limit: "This is more than the account's limit allows. Whoever manages the account can raise the limit.",
  always_not_available: "\"Always allow\" is not available for this request.",
};

/** `details.conflict` of a 422 `instance_host_conflict` (FIX-132): who holds the host. */
export interface HostConflict {
  kind: "static_claim" | "llm_host" | "aurora_address" | "instance";
  app_id?: string | null;
  app_name?: string | null;
  package_id?: string | null;
  endpoint_id?: string | null;
  setting?: string | null;
  scope?: "instance" | "org";
  instance_id?: string;
  /** (FIX-145, C-731) The holder's address: `https://<host><base_path>`. */
  base_path?: string | null;
  base_url?: string;
  promotable?: boolean;
  org?: { id: string; name?: string | null; slug?: string | null };
}

export function hostConflict(err: unknown): HostConflict | null {
  if (!(err instanceof ApiError) || err.code !== "instance_host_conflict") return null;
  const d = err.details;
  if (typeof d !== "object" || d === null || !("conflict" in d)) return null;
  const c = (d as { conflict: unknown }).conflict;
  if (typeof c !== "object" || c === null || !("kind" in c)) return null;
  return c as HostConflict;
}

/**
 * The sentence for a refused host: names the holder as far as the server says (an org administrator is
 * never told another organization's name). `ownOrg`: the registration was attempted in an organization.
 */
export function hostConflictMessage(c: HostConflict, ownOrg = false): string {
  const app = c.app_name ?? c.app_id ?? "another app";
  switch (c.kind) {
    case "static_claim":
      return `That host is claimed by the app ${app}${c.package_id ? ` (package ${c.package_id})` : ""}.`;
    case "llm_host":
      return c.endpoint_id
        ? `That host is the LLM endpoint ${c.endpoint_id}.`
        : "That host is an LLM endpoint of this instance.";
    case "aurora_address":
      return c.setting
        ? `That host is Aurora's own address (setting ${c.setting}).`
        : "That host is one of Aurora's own addresses.";
    case "instance": {
      // (D-231) The holder is named by its address when the server says it: another path on the
      // host may be free.
      const holder = c.base_url ? ` (${c.base_url})` : "";
      const what = c.base_url ? "That address overlaps" : "That host is already";
      const reg = c.base_url ? "one already registered as" : "registered as";
      if (c.scope !== "org") return `${what} ${reg} a self-hosted ${app}${holder} and is available to every organization.`;
      if (c.org?.name) return `${what} ${reg} a self-hosted ${app}${holder} by the organization ${c.org.name}.`;
      return ownOrg
        ? `${what} ${reg} a self-hosted ${app}${holder} in this organization.`
        : `${what} ${reg} a self-hosted ${app}${holder} by another organization — ask an instance administrator.`;
    }
  }
}

/** `details` of a 422 `instance_probe_failed` (FIX-144, C-730). Unknown `reason`s mean `unreachable`. */
export interface ProbeFailure {
  reason?: string;
  status?: number | null;
  url?: string;
  expected?: number[];
  location?: string;
}

export function probeFailure(err: unknown): ProbeFailure | null {
  if (!(err instanceof ApiError) || err.code !== "instance_probe_failed") return null;
  const d = err.details;
  return typeof d === "object" && d !== null ? (d as ProbeFailure) : {};
}

const REASON_PHRASES: Record<number, string> = {
  400: "Bad Request",
  401: "Unauthorized",
  403: "Forbidden",
  404: "Not Found",
  405: "Method Not Allowed",
  500: "Internal Server Error",
  502: "Bad Gateway",
  503: "Service Unavailable",
  504: "Gateway Timeout",
};

function urlParts(u: string | undefined): { host: string; path: string } | null {
  if (!u) return null;
  try {
    const p = new URL(u);
    return { host: p.host, path: p.pathname };
  } catch {
    return null;
  }
}

/** The sentence for a failed registration check: what the host actually answered (FIX-144). */
export function probeFailedMessage(d: ProbeFailure, appName: string, host: string, basePath?: string): string {
  const path = urlParts(d.url)?.path ?? "the check path";
  const app = appName || "The app";
  const status = d.status ?? undefined;
  const pathHint = `If ${app} is served under a path, such as https://${host}/jira, enter it in Path.`;
  switch (d.reason) {
    case "unexpected_status": {
      if (status === 404) {
        return basePath
          ? `${host} answered 404 Not Found at ${path}. ${app} answers there when it is served under ${basePath} on ${host}. Check the Path.`
          : `${host} answered 404 Not Found at ${path}. ${app} answers there when it is served at the root of the host. ${pathHint}`;
      }
      if (status !== undefined && status >= 300 && status < 400) {
        const loc = d.location ? urlParts(d.location) : null;
        const head = `${host} redirected ${path}${d.location ? ` to ${d.location}` : ""}.`;
        if (loc && loc.host.toLowerCase() !== host.toLowerCase()) return `${head} Register that host instead.`;
        return `${head} ${pathHint}`;
      }
      if (status !== undefined) {
        const phrase = REASON_PHRASES[status];
        const want = (d.expected ?? []).join(" or ");
        return `${host} answered ${status}${phrase ? ` ${phrase}` : ""} at ${path}${want ? `; ${app} answers ${want} there` : ""}. Check that this host runs ${app}.`;
      }
      break;
    }
    case "destination_refused":
      return `The gateway may not connect to ${host}: it resolves to a private address. An operator can allow it with gateway.allowed_private.`;
    case "dns":
      return `${host} could not be found from the Aurora server. Check the spelling. An internal name must resolve in the cluster's DNS.`;
    case "timeout":
      return `${host} did not answer within 10 seconds. Check that the Aurora server can reach it (firewall, proxy).`;
    case "connect":
      return `${host} refused the connection on port 443.`;
    case "tls":
      return `${host}'s TLS certificate is not trusted by Aurora. If a private CA signed it, an operator can add that CA (extra CA bundle).`;
  }
  return `${host} could not be reached from the Aurora server.`;
}

/** The one sentence for "no answer came" (C-915): never used for a refusal or any answer the server gave. */
export const NETWORK_SENTENCE = "Could not reach the server. Check your connection and try again.";

/**
 * (C-915) A failed call that never produced an answer: `fetch` itself failing (a `TypeError`), an aborted request, or
 * a 5xx that carried no error body (the client's `unknown`). Anything the server said is a real error.
 */
export function isTransportFailure(err: unknown): boolean {
  if (err instanceof ApiError) {
    return err.code === "network_error" || (err.code === "unknown" && (err.status === undefined || err.status === 0 || err.status >= 500));
  }
  if (err instanceof TypeError) return true;
  return typeof err === "object" && err !== null && (err as { name?: unknown }).name === "AbortError";
}

/** `details.cause` of a 422 `connect_probe_failed` (C-958): what the probe found, in words. */
export const CONNECT_PROBE_CAUSE: Record<string, string> = {
  rejected: "The app did not accept this token. Check that it is complete and has not expired.",
  unreachable: "Aurora could not reach the app. Check the address, and that Aurora may connect to it.",
  unexpected: "The app answered with an unexpected error.",
};

export function errorMessage(err: unknown): string {
  const conflict = hostConflict(err);
  if (conflict) return hostConflictMessage(conflict);
  if (err instanceof ApiError) {
    if (err.code === "connect_probe_failed") {
      const cause = (err.details as { cause?: unknown } | null)?.cause;
      if (typeof cause === "string" && CONNECT_PROBE_CAUSE[cause]) return CONNECT_PROBE_CAUSE[cause];
    }
    if (isTransportFailure(err)) return NETWORK_SENTENCE;
    return ERROR_MESSAGES[err.code] ?? (err.message || "Something went wrong.");
  }
  // A page gate's own error value, `{ code }` (C-915): its code is looked up like a server's, never read as "no answer".
  if (typeof err === "object" && err !== null && typeof (err as { code?: unknown }).code === "string") {
    const { code, message } = err as { code: string; message?: unknown };
    return ERROR_MESSAGES[code] ?? (typeof message === "string" && message ? message : "Something went wrong.");
  }
  return isTransportFailure(err) ? NETWORK_SENTENCE : "Something went wrong.";
}

/** `details[key]` of an API error, or undefined when it has no object details or no such key. */
export function errorDetail(err: unknown, key: string): unknown {
  const d = err instanceof ApiError ? err.details : undefined;
  return typeof d === "object" && d !== null ? (d as Record<string, unknown>)[key] : undefined;
}

/** The objects of `details.fields[]` (each `{ path?, message? }`); empty when there are none. */
export function detailFields(err: unknown): { path?: unknown; message?: unknown }[] {
  const fields = errorDetail(err, "fields");
  return Array.isArray(fields) ? fields.filter((f) => typeof f === "object" && f !== null) : [];
}

/** Field-level validation messages keyed by field path (`details.fields[]`). */
export function fieldErrors(err: unknown): Record<string, string> {
  const out: Record<string, string> = {};
  for (const { path, message } of detailFields(err)) {
    const key = typeof path === "string" ? path.replace(/^\//, "") : "";
    if (key && typeof message === "string" && !(key in out)) {
      out[key] = message;
    }
  }
  return out;
}

/** A react-query `retry` predicate: one retry, and none after a definitive client error (4xx). */
export const retryUnlessClientError = (count: number, err: unknown) =>
  !(err instanceof ApiError && err.status !== undefined && err.status >= 400 && err.status < 500) && count < 1;
