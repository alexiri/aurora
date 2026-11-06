import { ApiError } from "./client";

/** Friendly messages for M1 error codes (`docs/API_CONTRACTS.md`). */
export const ERROR_MESSAGES: Record<string, string> = {
  invalid_request: "Some of the values are not valid. Please check the form.",
  unauthorized: "Your session has expired. Please sign in again.",
  invalid_credentials: "The email or password is incorrect.",
  account_disabled: "This account has been disabled.",
  signup_closed: "Sign-up is closed on this instance.",
  email_taken: "An account with this email already exists.",
  rate_limited: "Too many requests. Please wait a moment and try again.",
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
  email_unverified: "Verify your email address first.",
  invalid_token: "This link is invalid, has already been used, or has expired.",
  already_verified: "Your email address is already verified.",
  reauthentication_required: "Please confirm your password to continue.",
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
};

export function errorMessage(err: unknown): string {
  if (err instanceof ApiError) {
    return ERROR_MESSAGES[err.code] ?? (err.message || "Something went wrong.");
  }
  return "Could not reach the server. Check your connection and try again.";
}

/** Field-level validation messages keyed by field path (`details.fields[]`). */
export function fieldErrors(err: unknown): Record<string, string> {
  const out: Record<string, string> = {};
  if (!(err instanceof ApiError)) return out;
  const d = err.details;
  if (typeof d !== "object" || d === null || !("fields" in d)) return out;
  const fields = (d as { fields: unknown }).fields;
  if (!Array.isArray(fields)) return out;
  for (const f of fields) {
    if (typeof f === "object" && f !== null) {
      const { path, message } = f as Record<string, unknown>;
      const key = typeof path === "string" ? path.replace(/^\//, "") : "";
      if (key && typeof message === "string" && !(key in out)) {
        out[key] = message;
      }
    }
  }
  return out;
}
