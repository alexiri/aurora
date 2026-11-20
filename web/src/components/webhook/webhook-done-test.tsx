"use client";

import { useState } from "react";

import { Button } from "@/components/ui/button";
import { Notice } from "@/components/ui/notice";
import { useWebhookTest, useWebhookView, isSent, type WebhookTestResult } from "@/lib/api/webhook";
import { initialInputs, valuesFromInputs } from "@/lib/webhook/logic";

import { testFailure } from "./send-test-dialog";
import { TestResult } from "./test-result";

/**
 * (P5-REFINE-13, access-4/-5) The Done step of the Outgoing webhook: "Not sure the address works?" with **Send a test**, then
 * what happened in words. The test sends the example message of the configuration just saved; nothing is typed.
 */
export function WebhookDoneTest({ connectionId, onEditAddress }: { connectionId: string; onEditAddress: () => void }) {
  const [asked, setAsked] = useState(false);
  const view = useWebhookView(connectionId, asked);
  const test = useWebhookTest(connectionId);
  const [result, setResult] = useState<WebhookTestResult | null>(null);
  const [failure, setFailure] = useState<string | null>(null);

  function send() {
    setResult(null);
    setFailure(null);
    setAsked(true);
    // The configuration (its slots and their examples) is read first; the values are the examples.
    void view.refetch().then((q) => {
      if (!q.data) {
        setFailure("Couldn't read the webhook. Try again.");
        return;
      }
      const { values, errors } = valuesFromInputs(q.data.slots, initialInputs(q.data.slots));
      if (Object.keys(errors).length > 0) {
        setFailure("This webhook needs values for its message. Open it from the account's menu and use Send a test there.");
        return;
      }
      test.mutate(
        { values, dry_run: false },
        {
          onSuccess: (r) => (isSent(r) ? setResult(r) : null),
          onError: (err) => setFailure(testFailure(err)),
        },
      );
    });
  }

  return (
    <div className="space-y-3 rounded-card bg-subtle p-4 text-left" data-testid="webhook-done-test" aria-live="polite">
      {result ? (
        <TestResult r={result} onAgain={send} onEdit={onEditAddress} />
      ) : (
        <>
          <p className="text-ink-2">Not sure the address works?</p>
          <Button type="button" variant="outline" disabled={test.isPending} onClick={send}>
            {test.isPending ? "Sending…" : "Send a test"}
          </Button>
          <p className="text-xs text-muted">This sends the example message to the address.</p>
        </>
      )}
      {failure ? (
        <Notice tone="danger" role="alert">
          <p className="font-semibold">The test did not go through</p>
          <p>{failure}</p>
          <Button type="button" size="sm" variant="ghost" onClick={onEditAddress}>
            Edit address
          </Button>
        </Notice>
      ) : null}
    </div>
  );
}
