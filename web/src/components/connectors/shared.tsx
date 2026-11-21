"use client";

import { channelError } from "@/components/channels/shared";
import { AppIcon } from "@/components/ui/app-icon";
import { AttentionChip } from "@/components/ui/attention-chip";
import { Badge } from "@/components/ui/badge";
import { buttonVariants } from "@/components/ui/button";
import { ErrorNotice, Notice } from "@/components/ui/notice";
import { ApiError } from "@/lib/api/client";
import { useMe } from "@/lib/api/queries";
import { orgRoutes } from "@/lib/routes";
import Link from "next/link";
import { connectorIconUrl, type ConnectorAdmin } from "@/lib/api/connectors";
import { fieldErrors } from "@/lib/api/errors";

const MESSAGES: Record<string, string> = {
  connector_exists: "A chat platform with this id already exists.",
  unknown_connector_type: "No running chat platform host offers this type any more.",
  connector_not_managed: "This chat platform is self-hosted, so its settings are not edited here.",
  connector_managed: "A managed chat platform has no token: Aurora runs it.",
  secret_store_unavailable: "The secret store is temporarily unavailable. Nothing was changed; try again in a moment.",
};

/**
 * (admin-5, C-1068 `next_step`) "Next: an organization owner connects Mattermost in their Chat platforms page." with a way
 * into each of the viewer's organizations, on the platforms list and on the platform's own page. Shown for `owner_connects`
 * and for `connect` (the viewer is such an owner: the sentence still names the role, D-407).
 */
export function ConnectorNextStep({ c }: { c: Pick<ConnectorAdmin, "display_name" | "next_step"> }) {
  const orgs = (useMe().data?.memberships ?? []).map((m) => m.org);
  if (c.next_step !== "owner_connects" && c.next_step !== "connect") return null;
  return (
    <Notice tone="accent" role="status" className="mt-1 flex flex-wrap items-center gap-x-4 gap-y-2" data-testid="connector-next">
      <p className="min-w-0 flex-1 basis-56">
        <strong className="font-semibold">Next:</strong> an organization owner connects {c.display_name} in their Chat platforms page.
      </p>
      {orgs.length ? (
        <span className="flex flex-wrap gap-2">
          {orgs.map((o) => (
            <Link key={o.slug} href={orgRoutes(o.slug).chatPlatforms()} className={buttonVariants({ variant: "outline", size: "sm" })}>
              Open {o.name}&apos;s Chat platforms
            </Link>
          ))}
        </span>
      ) : null}
    </Notice>
  );
}

/** Plain-text message for a connector admin error. */
export function connectorError(err: unknown): string {
  if (err instanceof ApiError && MESSAGES[err.code]) return MESSAGES[err.code];
  return channelError(err);
}

export function ConnectorError({ error }: { error: unknown }) {
  return <ErrorNotice error={error} format={connectorError} />;
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
  return <Badge>Unsigned (development)</Badge>;
}

/** Status of a connector for humans: managed ones wait for the host, show its error, or run. */
export function connectorStatus(c: ConnectorAdmin): { label: string; tone: "neutral" | "accent" | "danger" } {
  if (c.status === "disabled") return { label: "Disabled", tone: "neutral" };
  if (c.status === "pending_approval") return { label: "Needs approval", tone: "neutral" };
  const failing = c.host_status.find((h) => h.state === "error");
  if (c.managed && failing) {
    return { label: failing.message ? `Error: ${failing.message}` : "Error", tone: "danger" };
  }
  // (P5-REFINE-18 admin-5, C-1068) One readiness: a connector whose host is running is never "waiting for the host".
  if (c.status === "awaiting_registration" && c.managed && c.host_running) {
    return { label: "Connector host running", tone: "neutral" };
  }
  if (c.status === "awaiting_registration") {
    return { label: c.managed ? "Waiting for the chat platform host" : "Waiting for the chat platform", tone: "neutral" };
  }
  return { label: "Running", tone: "accent" };
}

export function ConnectorStatusChip({ c }: { c: ConnectorAdmin }) {
  const s = connectorStatus(c);
  return (
    <>
      <Badge data-testid="status-chip" tone={s.tone} className="max-w-full whitespace-normal break-words normal-case tracking-normal">
        {s.label}
      </Badge>
      {/* (FIX-167, D-369) A widened update waits while the connector keeps running at the approved level. */}
      {/* (P5-POLISH-6, D-384) Amber because it needs an admin: its action, Review, takes them to the approval below. */}
      {c.update_pending && c.status !== "pending_approval" ? (
        <AttentionChip
          data-testid="update-pending-chip"
          className="max-w-full whitespace-normal break-words"
          action={
            <a href={`#approval-${c.id}`} className="text-xs font-medium text-link underline">
              Review
            </a>
          }>
          Update waiting for approval
        </AttentionChip>
      ) : null}
    </>
  );
}

/** The connector's own icon; the monogram tile when it has none. */
export function ConnectorIcon({
  id,
  name,
  iconB64,
  hasIcon,
  size = 64,
}: {
  id: string;
  name: string;
  iconB64?: string | null;
  /** (C-1044) `ConnectorAdmin.has_icon`; required so no caller requests an icon that 404s. */
  hasIcon: boolean;
  size?: number;
}) {
  const src = iconB64 ? `data:image/png;base64,${iconB64}` : connectorIconUrl(id);
  return <AppIcon appId={id} name={name} src={src} hasIcon={hasIcon || !!iconB64} size={size} />;
}
