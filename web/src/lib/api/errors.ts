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
