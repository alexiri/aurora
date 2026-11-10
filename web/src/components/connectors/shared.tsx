"use client";

import { channelError } from "@/components/channels/shared";
import { AppIcon } from "@/components/ui/app-icon";
import { Badge } from "@/components/ui/badge";
import { Notice } from "@/components/ui/notice";
import { ApiError } from "@/lib/api/client";
import { connectorIconUrl, type ConnectorAdmin } from "@/lib/api/connectors";
import { fieldErrors } from "@/lib/api/errors";

export const count = (n: number, noun: string) => `${n} ${noun}${n === 1 ? "" : "s"}`;

export const fmtWhen = (s?: string | null) => (s ? new Date(s).toLocaleString() : "—");

const MESSAGES: Record<string, string> = {
  connector_exists: "A connector with this id already exists.",
  unknown_connector_type: "No running connector host offers this type any more.",
  connector_not_managed: "This connector is self-hosted, so its settings are not edited here.",
  connector_managed: "A managed connector has no token: Aurora runs it.",
  secret_store_unavailable: "The secret store is temporarily unavailable. Nothing was changed; try again in a moment.",
};

/** Plain-text message for a connector admin error. */
export function connectorError(err: unknown): string {
  if (err instanceof ApiError && MESSAGES[err.code]) return MESSAGES[err.code];
  return channelError(err);
}

export function ConnectorError({ error }: { error: unknown }) {
  if (!error) return null;
  return (
    <Notice tone="danger" role="alert">
      {connectorError(error)}
    </Notice>
  );
}

/**
 * Per-field API errors (`details.fields[].path`, e.g. `settings.server_url` or `/secrets/token`)
 * keyed by the bare field name; `names` limits the result to fields the form shows.
 */
export function settingErrors(err: unknown, names: string[]): Record<string, string> {
  const out: Record<string, string> = {};
  for (const [path, message] of Object.entries(fieldErrors(err))) {
    const name = path.split(/[./]/).pop() ?? path;
    if (names.includes(name) && !(name in out)) out[name] = message;
  }
  return out;
}

export function ManagedBadge({ managed }: { managed: boolean }) {
  return <Badge tone={managed ? "accent" : "neutral"}>{managed ? "Managed" : "Self-hosted"}</Badge>;
}

/** A managed type accepted without a signature under `plugins.allow_unsigned` (development only, D-142). */
export function UnsignedBadge() {
  return <Badge tone="pending">Unsigned (development)</Badge>;
}

/** Status of a connector for humans: managed ones wait for the host, show its error, or run. */
export function connectorStatus(c: ConnectorAdmin): { label: string; tone: "neutral" | "accent" | "pending" | "danger" } {
  if (c.status === "disabled") return { label: "Disabled", tone: "neutral" };
  if (c.status === "pending_approval") return { label: "Needs approval", tone: "pending" };
  const failing = c.host_status.find((h) => h.state === "error");
  if (c.managed && failing) {
    return { label: failing.message ? `Error: ${failing.message}` : "Error", tone: "danger" };
  }
  if (c.status === "awaiting_registration") {
    return { label: c.managed ? "Waiting for the connector host" : "Waiting for the connector", tone: "neutral" };
  }
  return { label: "Running", tone: "accent" };
}

export function ConnectorStatusChip({ c }: { c: ConnectorAdmin }) {
  const s = connectorStatus(c);
  return (
    <Badge data-testid="status-chip" tone={s.tone} className="max-w-full whitespace-normal break-words normal-case tracking-normal">
      {s.label}
    </Badge>
  );
}

/** The connector's own icon; the monogram tile when it has none. */
export function ConnectorIcon({
  id,
  name,
  iconB64,
  hasIcon = true,
  size = 40,
}: {
  id: string;
  name: string;
  iconB64?: string | null;
  hasIcon?: boolean;
  size?: number;
}) {
  const src = iconB64 ? `data:image/png;base64,${iconB64}` : connectorIconUrl(id);
  return <AppIcon appId={id} name={name} src={src} hasIcon={hasIcon || !!iconB64} size={size} />;
}
