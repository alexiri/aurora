"use client";

import { useMemo, useState } from "react";

import { ErrorState, Loading } from "@/components/states";
import { Button } from "@/components/ui/button";
import { ConfirmDialog } from "@/components/ui/confirm-dialog";
import { Dialog, DialogContent } from "@/components/ui/dialog";
import { Notice } from "@/components/ui/notice";
import { ApiError } from "@/lib/api/client";
import { errorMessage } from "@/lib/api/errors";
import { isSent, useWebhookTest, useWebhookView, type WebhookPreview, type WebhookTestResult, type WebhookView } from "@/lib/api/webhook";
import { initialInputs, invalidSlot, valuesFromInputs } from "@/lib/webhook/logic";

import { SlotValues } from "./slot-values";

/** The refusal sentences of a test: the rate limit with its wait, the org switch, the others in the standard words. */
export function testFailure(err: unknown): string {
  if (err instanceof ApiError && err.code === "webhook_rate_limited") {
    const wait = (err.details as { retry_after_secs?: unknown } | null)?.retry_after_secs;
    return typeof wait === "number" ? `This webhook's limit was reached. Try again in ${wait} second${wait === 1 ? "" : "s"}.` : "This webhook's limit was reached. Try again in a little while.";
  }
  if (err instanceof ApiError && err.code === "outgoing_webhooks_disabled") return "Outgoing webhooks are turned off for this organization.";
  return errorMessage(err);
}

function Result({ r }: { r: WebhookTestResult | WebhookPreview }) {
  if (isSent(r)) {
    return (
      <div className="space-y-2" role="status" data-testid="test-result">
        <p className="text-sm font-medium text-ink">The receiver answered {r.status}.</p>
        <dl className="grid grid-cols-[auto_1fr] gap-x-3 gap-y-1 text-sm">
          <dt className="text-muted">Delivery id</dt>
          <dd className="break-all font-mono text-xs text-ink">{r.delivery_id}</dd>
          {r.content_type ? (
            <>
              <dt className="text-muted">Content type</dt>
              <dd className="break-all text-ink">{r.content_type}</dd>
            </>
          ) : null}
        </dl>
        {/* The answer is text: shown inside <pre> as a string, never as HTML. */}
        <pre data-testid="test-body" className="max-h-64 overflow-auto whitespace-pre-wrap break-all rounded-control border border-line bg-canvas p-3 font-mono text-xs text-ink">
          {r.body || "(empty answer)"}
        </pre>
        {r.truncated ? <p className="text-xs text-muted">The answer was longer than 4 KiB; the rest is not shown.</p> : null}
      </div>
    );
  }
  return (
    <div className="space-y-2" role="status" data-testid="test-result">
      <p className="text-sm font-medium text-ink">Dry run: nothing was sent.</p>
      <p className="text-sm text-ink-2">
        {r.method} as {r.content_type}
        {r.headers.length > 0 ? `, with the headers ${r.headers.join(", ")}` : ""}.
      </p>
      <pre data-testid="test-body" className="max-h-64 overflow-auto whitespace-pre-wrap break-all rounded-control border border-line bg-canvas p-3 font-mono text-xs text-ink">
        {r.body}
      </pre>
    </div>
  );
}

function TestForm({ view, connectionId, name }: { view: WebhookView; connectionId: string; name: string }) {
  const test = useWebhookTest(connectionId);
  const [typed, setTyped] = useState<Record<string, string>>({});
  const [errors, setErrors] = useState<Record<string, string>>({});
  const [confirm, setConfirm] = useState(false);
  const [result, setResult] = useState<WebhookTestResult | WebhookPreview | null>(null);
  const [failure, setFailure] = useState<string | null>(null);
  const inputs = useMemo(() => ({ ...initialInputs(view.slots), ...typed }), [view.slots, typed]);

  function run(dry: boolean) {
    const { values, errors: local } = valuesFromInputs(view.slots, inputs);
    setErrors(local);
    setFailure(null);
    if (Object.keys(local).length > 0) return;
    setResult(null);
    test.mutate(
      { values, dry_run: dry },
      {
        onSuccess: setResult,
        onError: (err) => {
          const slot = invalidSlot(err);
          if (slot) setErrors({ [slot.slot]: slot.message });
          else setFailure(testFailure(err));
        },
        onSettled: () => setConfirm(false),
      },
    );
  }

  return (
    <div className="space-y-4">
      <p className="text-sm text-ink-2">
        Sends to <span className="font-medium text-ink">{view.host}</span> with the values below. A dry run only shows what would be sent.
      </p>
      <SlotValues slots={view.slots} inputs={inputs} errors={errors} onChange={(n, v) => setTyped((t) => ({ ...t, [n]: v }))} idp="st" />
      <div className="flex flex-wrap gap-2">
        <Button type="button" variant="outline" disabled={test.isPending} onClick={() => run(true)}>
          Dry run
        </Button>
        <Button
          type="button"
          disabled={test.isPending}
          onClick={() => {
            const { errors: local } = valuesFromInputs(view.slots, inputs);
            setErrors(local);
            if (Object.keys(local).length === 0) setConfirm(true);
          }}
        >
          {test.isPending ? "Working…" : "Send"}
        </Button>
      </div>
      {failure ? (
        <Notice tone="danger" role="alert">
          {failure}
        </Notice>
      ) : null}
      {result ? <Result r={result} /> : null}
      <ConfirmDialog
        open={confirm}
        onOpenChange={setConfirm}
        title={`Send a test to ${name}?`}
        description="This really calls the webhook. The receiver will act on it."
        confirmLabel="Send"
        busy={test.isPending}
        onConfirm={() => run(false)}
      />
    </div>
  );
}

/** Send test (C-913): a values form from the slots, **Dry run** and **Send** (after a confirmation), then the answer. */
export function SendTestDialog({ connectionId, name, open, onOpenChange }: { connectionId: string; name: string; open: boolean; onOpenChange: (o: boolean) => void }) {
  const q = useWebhookView(connectionId, open);
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent size="lg" title={`Send test: ${name}`}>
        {!open ? null : q.isError ? (
          <ErrorState error={q.error} onRetry={() => void q.refetch()} />
        ) : !q.data || !q.isFetchedAfterMount ? (
          <Loading />
        ) : (
          <TestForm view={q.data} connectionId={connectionId} name={name} />
        )}
      </DialogContent>
    </Dialog>
  );
}
