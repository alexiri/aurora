import { ApiError } from "@/lib/api/client";

/** The shape of an LLM secret value per auth mode (C-801, C-802 `value_shape`). */
export type ValueShape = "string" | "aws_sigv4" | "aws_assume_role" | "azure_entra" | "gcp_service_account";

/** One field of a structured secret value. */
export interface KeyField {
  name: string;
  label: string;
  hint?: string;
  /** Secret fields are write-only password inputs; the others are plain text. */
  secret?: boolean;
  optional?: boolean;
  /** Sent as a number (`session_duration_secs`). */
  numeric?: boolean;
}

/** The fields of each structured shape, in the order the form shows them. */
export const SHAPE_FIELDS: Record<Exclude<ValueShape, "string" | "gcp_service_account">, KeyField[]> = {
  aws_sigv4: [
    { name: "access_key_id", label: "Access key id" },
    { name: "secret_access_key", label: "Secret access key", secret: true },
    { name: "session_token", label: "Session token", secret: true, optional: true, hint: "Only for temporary credentials." },
  ],
  aws_assume_role: [
    { name: "role_arn", label: "Role ARN", hint: "arn:aws:iam::<account id>:role/<name>" },
    { name: "access_key_id", label: "Source access key id", hint: "The IAM user that is allowed to assume the role." },
    { name: "secret_access_key", label: "Source secret access key", secret: true },
    {
      name: "session_duration_secs",
      label: "Session length in seconds",
      optional: true,
      numeric: true,
      hint: "900 to 43200. Default 3600.",
    },
  ],
  azure_entra: [
    { name: "tenant_id", label: "Tenant id", hint: "The directory (tenant) GUID; common, organizations and consumers are not accepted." },
    { name: "client_id", label: "Client id", hint: "The application (client) GUID." },
    { name: "client_secret", label: "Client secret", secret: true },
  ],
};

/** The name of an auth mode in the form (C-811): "Assume a role", "Access keys", "Bedrock API key", … */
export function modeLabel(mode: string, provider: string | undefined): string {
  switch (mode) {
    case "api_key":
      if (provider === "azure_openai") return "Resource key";
      if (provider === "bedrock") return "Bedrock API key";
      return "API key";
    case "aws_sigv4":
      return "Access keys";
    case "aws_assume_role":
      return "Assume a role";
    case "azure_entra":
      return "Microsoft Entra application";
    case "gcp_service_account":
      return "Service account key file";
    default:
      return mode;
  }
}

/** A sentence under the mode: what the choice is and when to use it. */
export function modeHelp(mode: string, provider: string | undefined): string | undefined {
  switch (mode) {
    case "aws_assume_role":
      return "Aurora assumes an IAM role in your account with short-lived credentials. The role is checked at save time, and only the account is shown afterwards.";
    case "aws_sigv4":
      return "A long-lived IAM access key. Limit it to bedrock:InvokeModel*. Only the account id is shown afterwards.";
    case "azure_entra":
      return "An app registration in Microsoft Entra with access to the Azure OpenAI resource. Only the tenant and application are shown afterwards.";
    case "gcp_service_account":
      return "The JSON key file of a service account that may call Vertex AI. The private key is never shown again; the project and service account are.";
    case "api_key":
      if (provider === "azure_openai") return "One of the resource's two keys.";
      if (provider === "bedrock") return "A Bedrock API key from the AWS console.";
      return undefined;
    default:
      return undefined;
  }
}

/** The trust policy a role must carry for Aurora to assume it: this organization's external id is the condition (C-753). */
export function trustPolicy(externalId: string): string {
  return JSON.stringify(
    {
      Version: "2012-10-17",
      Statement: [
        {
          Effect: "Allow",
          Principal: { AWS: "arn:aws:iam::<the account of the source access key>:user/<source user>" },
          Action: "sts:AssumeRole",
          Condition: { StringEquals: { "sts:ExternalId": externalId } },
        },
      ],
    },
    null,
    2,
  );
}

/** What the create request's `value` is for a mode: the string itself, or the structured value as JSON (C-801). */
export function buildValue(shape: ValueShape, text: string, fields: Record<string, string>): string {
  if (shape === "string" || shape === "gcp_service_account") return text;
  const out: Record<string, string | number> = {};
  for (const f of SHAPE_FIELDS[shape]) {
    const v = (fields[f.name] ?? "").trim();
    if (!v) continue;
    out[f.name] = f.numeric ? Number(v) : v;
  }
  return JSON.stringify(out);
}

/** Is every required field of the mode filled? */
export function valueComplete(shape: ValueShape, text: string, fields: Record<string, string>): boolean {
  if (shape === "string") return text.trim().length > 0;
  if (shape === "gcp_service_account") return text.trim().length > 0;
  return SHAPE_FIELDS[shape].every((f) => f.optional || (fields[f.name] ?? "").trim().length > 0);
}

/** `value_shape` of a mode from the provider's `auth_mode_details`; an unlisted mode is a plain string. */
export function shapeOf(details: { id: string; value_shape: string }[] | undefined, mode: string): ValueShape {
  const s = details?.find((d) => d.id === mode)?.value_shape;
  return s === "aws_sigv4" || s === "aws_assume_role" || s === "azure_entra" || s === "gcp_service_account" ? s : "string";
}

/** C-801 `details.reason` of a 422 `llm_key_invalid`, in plain language. */
export const KEY_REASON: Record<string, string> = {
  auth_mode_unsupported: "This endpoint does not accept that way of signing in. Choose another.",
  missing_field: "A required field is empty.",
  unknown_field: "The value has a field this mode does not use. Check that you chose the right mode.",
  role_arn_invalid: "That is not a role ARN. It looks like arn:aws:iam::123456789012:role/name.",
  partition_mismatch: "The role is in a different AWS partition than the endpoint's region (for example a GovCloud role for a commercial region).",
  duration_out_of_range: "The session length must be between 900 and 43200 seconds.",
  tenant_not_specific: "Use the tenant's own id. common, organizations and consumers are not accepted.",
  token_uri_not_google: "This key file does not point at Google's token service, so Aurora will not use it.",
  not_service_account: "This file is not a service account key. Download a JSON key for a service account.",
  assume_role_denied:
    "AWS refused to let this key assume the role. Check the role's trust policy: it must allow the source user and require this organization's external id.",
  mint_rejected: "The provider did not accept these credentials. Check them and try again.",
  provider_rejected: "The provider rejected this key.",
};

/** The plain-language reason of an `llm_key_invalid` error, if it carries one we know. */
export function keyReasonMessage(err: unknown): string | undefined {
  if (!(err instanceof ApiError) || err.code !== "llm_key_invalid") return undefined;
  const d = err.details;
  if (typeof d !== "object" || d === null) return undefined;
  const reason = (d as { reason?: unknown }).reason;
  return typeof reason === "string" ? KEY_REASON[reason] : undefined;
}

/** Display names of the harness ids an endpoint serves (C-802). */
export const HARNESS_LABEL: Record<string, string> = {
  claude_code: "Claude Code",
  "claude-code": "Claude Code",
  codex: "Codex",
  pi: "Pi",
  goose: "goose",
  opencode: "OpenCode",
};

export const NO_HARNESS =
  "No enabled harness speaks this endpoint's API — use a LiteLLM endpoint to translate.";

/** "Works with Claude Code, Codex" — empty when the endpoint serves none. */
export function harnessHint(harnesses: string[]): string {
  return harnesses.length === 0 ? "" : `Works with ${harnesses.map((h) => HARNESS_LABEL[h] ?? h).join(", ")}`;
}
