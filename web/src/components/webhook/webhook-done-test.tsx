"use client";

import { useState } from "react";

import { Button } from "@/components/ui/button";
import { Notice } from "@/components/ui/notice";
import { useWebhookTest, useWebhookView, isSent, type Slot, type WebhookTestResult } from "@/lib/api/webhook";
import { initialInputs, valuesFromInputs } from "@/lib/webhook/logic";

import { SlotValues } from "./slot-values";
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
  const [slots, setSlots] = useState<Slot[] | null>(null);
  const [inputs, setInputs] = useState<Record<string, string>>({});
  const [errors, setErrors] = useState<Record<string, string>>({});

  function post(values: Record<string, unknown>) {
    test.mutate(
      { values, dry_run: false },
      {
        onSuccess: (r) => {
          if (isSent(r)) {
            setSlots(null);
            setResult(r);
          }
        },
        onError: (err) => setFailure(testFailure(err)),
      },
    );
  }

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
      const start = initialInputs(q.data.slots);
      const { values, errors: local } = valuesFromInputs(q.data.slots, start);
      if (Object.keys(local).length > 0) {
        // Some value has no example: ask for the values here, in place.
        setSlots(q.data.slots);
        setInputs(start);
        setErrors({});
        return;
      }
      post(values);
    });
  }

  function sendTyped() {
    if (!slots) return;
    const { values, errors: local } = valuesFromInputs(slots, inputs);
    setErrors(local);
    setFailure(null);
    if (Object.keys(local).length === 0) post(values);
  }

  return (
    <div className="space-y-3 rounded-card bg-subtle p-4 text-left" data-testid="webhook-done-test" aria-live="polite">
      {result ? (
        <TestResult r={result} onAgain={send} onEdit={onEditAddress} />
      ) : slots ? (
        <>
          <p className="text-ink-2">This webhook needs a few values for its message. Fill them in and send the test.</p>
          <SlotValues slots={slots} inputs={inputs} errors={errors} onChange={(n, v) => setInputs((t) => ({ ...t, [n]: v }))} idp="done-test" />
          <Button type="button" disabled={test.isPending} onClick={sendTyped}>
            {test.isPending ? "Sending…" : "Send a test"}
          </Button>
        </>
      ) : (
        <>
          <p className="text-ink-2">Not sure the address works?</p>
          <Button type="button" variant="outline" disabled={test.isPending} onClick={send}>
            {test.isPending ? "Sending…" : "Send a test"}
          </Button>
          <p className="text-xs text-muted">This sends the example message to the address.</p>
        </>
      )}
      <p className="text-xs text-muted">An address can&apos;t be changed once saved: Edit address makes a new account with the corrected one and removes this one.</p>
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
