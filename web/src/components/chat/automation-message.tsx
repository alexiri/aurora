"use client";

import { Altitude } from "@/components/ui/altitude";
import { TechnicalDetails } from "@/components/ui/technical-details";
import { formatTime } from "@/lib/format-date";

/** What the API puts in the `display` of a schedule or webhook turn's `turn.queued` event (C-1079). */
export type AutomationDisplay =
  | { kind: "webhook"; name: string; deliveryId: string; contentType: string; size: number | null; truncated: boolean; body: string; verification: "signed" | "token" | "none" | null }
  | { kind: "schedule"; name: string; scheduleId: string; runId: string };

const str = (v: unknown): string => (typeof v === "string" ? v : "");

/** Reads `payload.display` defensively (old events have none, and nothing here may throw on an unexpected shape). */
export function automationDisplay(payload: Record<string, unknown>): AutomationDisplay | null {
  const d = payload.display;
  if (!d || typeof d !== "object") return null;
  const o = d as Record<string, unknown>;
  if (o.kind === "webhook") {
    return {
      kind: "webhook",
      name: str(o.name),
      deliveryId: str(o.delivery_id),
      contentType: str(o.content_type),
      size: typeof o.size === "number" ? o.size : null,
      truncated: o.truncated === true,
      body: str(o.body),
      // (C-1097) How the sender was checked; old events have none.
      verification: o.verification === "signed" || o.verification === "token" || o.verification === "none" ? o.verification : null,
    };
  }
  if (o.kind === "schedule") return { kind: "schedule", name: str(o.name), scheduleId: str(o.schedule_id), runId: str(o.run_id) };
  return null;
}

/** (webhooks-4, C-1097) "signed and accepted"; a token: "accepted with its token"; nothing checked: "accepted, not signed". */
export function acceptedWords(v: Extract<AutomationDisplay, { kind: "webhook" }>["verification"]): string {
  if (v === "signed") return "signed and accepted";
  if (v === "token") return "accepted with its token";
  if (v === "none") return "accepted, not signed";
  return "accepted";
}

/**
 * (P5-REFINE-14, webhooks-4, D-409) The first message of a run's or delivery's thread, as people read it: "Webhook Deploy notice ·
 * delivery" with the time under it and the payload in a code block, or "Schedule Morning summary · run". The safety wrapper and
 * the delivery id are never rendered (the agent still receives them; the API sends this display separately); the ids sit in a
 * collapsed Technical details for Expert.
 */
export function AutomationMessage({ display, ts, text }: { display: AutomationDisplay; ts: string; text?: string }) {
  // 24-hour, in the person's zone like every other time on the screen.
  const time = formatTime(ts, "");
  if (display.kind === "schedule") {
    return (
      <div className="space-y-1 break-words" data-testid="automation-message">
        <p className="font-semibold">Schedule {display.name} · run</p>
        {time ? <p className="text-xs opacity-80">{time}</p> : null}
        {text?.trim() ? <p className="whitespace-pre-wrap">{text}</p> : null}
      </div>
    );
  }
  return (
    <div className="space-y-2" data-testid="automation-message">
      <div>
        <p className="font-semibold">Webhook {display.name} · delivery</p>
        <p className="text-xs opacity-80">{time ? `${time} · ${acceptedWords(display.verification)}` : acceptedWords(display.verification)}</p>
      </div>
      {display.body ? (
        <pre className="max-w-full overflow-x-auto whitespace-pre-wrap break-all rounded-control bg-subtle p-2.5 font-mono text-xs text-ink">{display.body}</pre>
      ) : null}
      {display.truncated ? <p className="text-xs opacity-80">The payload was long, so only the start is shown.</p> : null}
      <Altitude id="auto.deliveryids" level="expert">
        {/* On the bubble's own background `text-muted` fails contrast (3.1:1 light, 4.4:1 dark): the bubble's token. */}
        <TechnicalDetails className="text-xs" summaryClassName="text-bubble-muted" bodyClassName="mt-1 break-all font-mono text-bubble-ink">
          {[display.deliveryId && `Delivery ${display.deliveryId}`, display.contentType, display.size !== null ? `${display.size} B` : ""].filter(Boolean).join(" · ")}
        </TechnicalDetails>
      </Altitude>
    </div>
  );
}
