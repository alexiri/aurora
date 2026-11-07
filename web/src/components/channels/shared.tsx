"use client";

import { ErrorState } from "@/components/states";
import { ApiError } from "@/lib/api/client";
import { errorMessage } from "@/lib/api/errors";
import type { FormField } from "@/lib/api/connectors";
import { cn } from "@/lib/utils";

/** Messages for the M6 error codes (API_CONTRACTS C-31..C-38). */
const CHANNEL_MESSAGES: Record<string, string> = {
  tenant_already_bound: "This workspace of the chat platform is already connected to another organization.",
  connector_unavailable: "The connector did not answer in time. Try again in a moment.",
  connector_error: "The connector reported an error.",
  no_shared_bot: "This integration has no shared bot. Use a dedicated identity instead.",
  mode_unsupported: "This connector does not support that mode.",
  link_expired: "This link has expired or was already used. Ask for a new one in the chat.",
  already_linked: "That chat account is already linked to another Aurora user.",
  agent_not_reachable: "You cannot reach that agent from this integration.",
  stale_revision: "This request changed in the meantime. The latest version has been loaded.",
  stale_manifest: "The connector announced a different manifest. The latest version has been loaded; review it again.",
  connector_in_use: "Integrations still use this connector. Remove them first.",
  conflict: "That conflicts with the current state. Reload and try again.",
};

/** Plain-text message for a channel error; connector messages are shown as text only. */
export function channelError(err: unknown): string {
  if (err instanceof ApiError) {
    if (err.code === "credentials_invalid") {
      return `The platform rejected the credentials${err.message ? `: ${err.message}` : "."}`;
    }
    if (err.code === "connector_error" && err.message) return `${CHANNEL_MESSAGES.connector_error} ${err.message}`;
    const m = CHANNEL_MESSAGES[err.code];
    if (m) return m;
  }
  return errorMessage(err);
}

export function MutationError({ error }: { error: unknown }) {
  if (!error) return null;
  return (
    <p role="alert" className="text-sm text-red-700">
      {channelError(error)}
    </p>
  );
}

export function isStatus(err: unknown, code: string): boolean {
  return err instanceof ApiError && err.code === code;
}

const TONE: Record<string, string> = {
  active: "bg-green-100 text-green-800",
  approved: "bg-green-100 text-green-800",
  validating: "bg-amber-100 text-amber-800",
  pending_setup: "bg-amber-100 text-amber-800",
  pending: "bg-amber-100 text-amber-800",
  pending_approval: "bg-amber-100 text-amber-800",
  awaiting_registration: "bg-neutral-100 text-neutral-700",
  needs_attention: "bg-red-100 text-red-800",
  blocked: "bg-red-100 text-red-800",
  disabled: "bg-neutral-200 text-neutral-700",
};

export function StatusChip({ status }: { status: string }) {
  return (
    <span
      data-testid="status-chip"
      className={cn("rounded-full px-2 py-0.5 text-xs", TONE[status] ?? "bg-neutral-100 text-neutral-700")}
    >
      {status.replace(/_/g, " ")}
    </span>
  );
}

/** Credentials-store fields are write-only secrets; everything else is plain config. */
export function splitByStore(fields: FormField[], values: Record<string, string>) {
  const config: Record<string, string> = {};
  const credentials: Record<string, string> = {};
  for (const f of fields) {
    const v = values[f.name];
    if (v === undefined || v === "") continue;
    (f.store === "credentials" ? credentials : config)[f.name] = v;
  }
  return { config, credentials };
}

export function Forbidden() {
  return <ErrorState error={{ code: "forbidden" }} />;
}
