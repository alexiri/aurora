"use client";

import Link from "next/link";
import { useParams } from "next/navigation";

import { Button } from "@/components/ui/button";
import { WhoCan } from "@/components/ui/who-can";
import { useDismissAccessRequest } from "@/lib/api/access-requests";
import type { TranscriptEvent } from "@/lib/api/conversations";
import { useWorkspace } from "@/lib/api/queries";
import { useWhoCan } from "@/lib/api/who-can";
import { wsRoutes } from "@/lib/routes";

import type { AgentIdent } from "./types";

const s = (v: unknown): string => (typeof v === "string" ? v : "");

/** The notice codes the conversation shows as the request card, and the two that settle it. */
export const ACCESS_REQUEST_CODES = ["app_connection_requested", "secret_proposed"] as const;
export const ACCESS_SETTLED_CODES = ["app_access_given", "app_access_request_dismissed"] as const;

/** How a request ended, read from the transcript: a notice of the same request. */
export function accessRequestOutcome(events: TranscriptEvent[], requestId: string): { kind: "given" | "dismissed"; text: string } | null {
  if (!requestId) return null;
  for (const e of events) {
    if (e.type !== "notice" || s(e.payload.request_id) !== requestId) continue;
    const code = s(e.payload.code);
    if (code === "app_access_given") return { kind: "given", text: s(e.payload.text) };
    if (code === "app_access_request_dismissed") return { kind: "dismissed", text: "Dismissed." };
  }
  return null;
}

/**
 * (P5-UX-17, C-984, D-348) An agent's request for an app (or a token for a website) as a card in the conversation:
 * "Daneel needs Linear for this", the agent's reason marked as its own words, **Give Daneel Linear** (the journey for
 * that agent and app) or who can, and **Not now**. Once the access was given the card is the Done sentence; once
 * dismissed, "Dismissed.". The reason is the agent's text and is shown as a quotation, never as the platform's.
 */
export function AccessRequestCard({
  ev,
  events,
  wsId,
  agent,
}: {
  ev: TranscriptEvent;
  events: TranscriptEvent[];
  wsId: string;
  agent?: AgentIdent;
}) {
  const p = ev.payload;
  const requestId = s(p.request_id);
  const params = useParams<{ org?: string; ws?: string }>();
  const who = useWhoCan({ workspaceId: wsId }, "workspace_owner", "give_access");
  const canOwn = useWorkspace(wsId).data?.my_role === "owner";
  const dismiss = useDismissAccessRequest(wsId);
  const outcome = accessRequestOutcome(events, requestId);
  const website = s(p.code) === "secret_proposed";
  const name = agent?.name ?? "The agent";
  const target = website ? s(p.host) : s(p.app_name) || s(p.app_id);
  const routes = wsRoutes(params?.org ?? "", params?.ws ?? "");
  const appId = s(p.app_id);
  const journey = routes.agent(agent?.id ?? "", s(p.connect_url).includes("connect=") && appId ? { tab: "access", connect: appId } : { tab: "access" });
  const tokenHref = routes.accounts(undefined, "requests");

  // Settled: the card folds away; the conversation carries the Done sentence (`app_access_given`) or "Dismissed.".
  if (outcome) return null;
  return (
    <div className="max-w-chat-card space-y-2 rounded-card rounded-tl-sm border border-pending-line bg-pending-panel p-3 text-sm text-ink" data-testid="access-request-card">
      <p className="break-words font-semibold">{website ? `${name} needs a token for ${target}` : `${name} needs ${target} for this`}</p>
      {s(p.reason) ? (
        <p className="break-words text-ink-2">
          <span className="text-xs font-medium text-muted">{name}&apos;s words: </span>“{s(p.reason)}”
        </p>
      ) : null}
      {canOwn ? (
        <div className="flex flex-wrap items-center gap-2">
          <Button asChild size="sm">
            <Link href={website ? tokenHref : journey}>{website ? "Add the token" : `Give ${name} ${target}`}</Link>
          </Button>
          <Button size="sm" variant="ghost" disabled={dismiss.isPending || !requestId} onClick={() => dismiss.mutate(requestId)}>
            Not now
          </Button>
          {dismiss.isError ? <span role="alert" className="text-xs text-danger-ink">Could not dismiss it.</span> : null}
        </div>
      ) : (
        <WhoCan value={who} />
      )}
    </div>
  );
}
