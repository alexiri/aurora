"use client";

import { Altitude } from "@/components/ui/altitude";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";
import { webhookTestSentence } from "@/lib/labels";
import type { WebhookTestResult } from "@/lib/api/webhook";

/** The receiver's own words: at most this many characters of its answer are shown, as text. */
export const ANSWER_SHOWN = 200;

const REASON: Record<number, string> = { 200: "OK", 201: "Created", 202: "Accepted", 204: "No Content", 400: "Bad Request", 401: "Unauthorized", 403: "Forbidden", 404: "Not Found", 405: "Method Not Allowed", 410: "Gone", 429: "Too Many Requests", 500: "Internal Server Error", 502: "Bad Gateway", 503: "Service Unavailable", 504: "Gateway Timeout" };

/** "Answer 405 Method Not Allowed". */
export function answerLine(status: number): string {
  return REASON[status] ? `Answer ${status} ${REASON[status]}` : `Answer ${status}`;
}

/** The delivery id cut to its first characters: "01a1…". */
function shortId(id: string): string {
  return id.length > 6 ? `${id.slice(0, 4)}…` : id;
}

/**
 * (P5-REFINE-13, access-5) What a test of the address did, in words. A 2xx is a green line; anything else is a red result with
 * one plain sentence. **Details** (collapsed) holds the answer code, the delivery id and the first 200 characters of what
 * the receiver sent back: shown as text in a `<pre>`, never as HTML.
 */
export function TestResult({ r, onAgain, onEdit }: { r: WebhookTestResult; onAgain?: () => void; onEdit?: () => void }) {
  const ok = r.status >= 200 && r.status < 300;
  const body = r.body ?? "";
  const cut = body.length > ANSWER_SHOWN;
  return (
    <div
      role="status"
      data-testid="test-result"
      className={cn("space-y-3 rounded-card border p-4 text-sm", ok ? "border-line bg-subtle" : "border-danger-ink/30 bg-danger-soft")}
    >
      {ok ? (
        <p className="font-medium text-ink">The test went through.</p>
      ) : (
        <>
          <p className="font-semibold text-danger-ink">The test did not go through</p>
          <p className="text-danger-ink">{webhookTestSentence(r.status)}</p>
        </>
      )}
      <details className={ok ? "text-ink-2" : "text-danger-ink"}>
        <summary className="cursor-pointer">Details</summary>
        {/* The answer is text: shown inside <pre> as a string, never as HTML. */}
        <pre data-testid="test-body" className="mt-2 max-h-64 overflow-auto whitespace-pre-wrap break-all rounded-control border border-line bg-canvas p-3 font-mono text-xs text-ink">
          <Altitude id="auto.deliveryids" level="expert" promote={!ok}>
            {answerLine(r.status)}
            {r.delivery_id ? ` · delivery ${shortId(r.delivery_id)}` : ""}
            {"\n"}
          </Altitude>
          {body ? (
            <>
              {body.slice(0, ANSWER_SHOWN)}
              {cut ? <span className="text-muted">… (first {ANSWER_SHOWN} characters)</span> : null}
            </>
          ) : (
            "(empty answer)"
          )}
        </pre>
      </details>
      {onAgain || onEdit ? (
        <div className="flex flex-wrap items-center gap-2">
          {onAgain ? (
            <Button type="button" variant="outline" onClick={onAgain}>
              Send test again
            </Button>
          ) : null}
          {onEdit && !ok ? (
            <Button type="button" variant="ghost" onClick={onEdit}>
              Edit address
            </Button>
          ) : null}
        </div>
      ) : null}
    </div>
  );
}
